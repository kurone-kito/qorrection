//! Assemble the host↔child I/O pump over a [`SpawnedSession`].
//!
//! [`start_io_pump`] consumes the master writer and clones the
//! master reader from a live PTY spawn, then hands them to
//! [`super::forward::spawn_forwarder`] in both directions. The
//! returned [`IoPump`] is a passive bundle of two
//! [`super::forward::ForwarderHandle`]s; the wait/drain
//! supervisor that converges them with the child wait is owned
//! by PR 4 (#33).

use std::io::{Read, Write};

use crate::pty::forward::{spawn_forwarder, Direction, ForwarderHandle};
use crate::pty::spawn::SpawnedSession;
use crate::trigger::{
    input::{shared_input_pump, InputDetector},
    output::OutputArbiter,
};
use crate::{Error, Result};

/// Owning bundle of the host↔child forwarder threads.
///
/// Direction tags are preserved so PR 4's supervisor can
/// attribute join failures (and decide drain ordering) without
/// relying on field position.
pub(crate) struct IoPump {
    pub(crate) host_to_child: ForwarderHandle,
    pub(crate) child_to_host: ForwarderHandle,
}

/// Wire host stdio onto a live `SpawnedSession` and spawn both
/// forwarder threads.
///
/// **Acquisition order matters**: the master reader is cloned
/// FIRST, then the one-shot writer is taken. If we took the
/// writer first and the subsequent `try_clone_reader()` failed,
/// dropping that writer on the error path would prematurely
/// signal EOF to the child — leaking a half-shutdown to the
/// caller. Cloning first keeps `take_writer()` the single
/// committing step.
///
/// Errors from portable-pty (handle acquisition, fd dup) flow
/// through [`Error::Pty`], preserving the existing exit-code
/// classification from `src/error.rs`.
pub(crate) fn start_io_pump<HIn, HOut>(
    session: &mut SpawnedSession,
    host_stdin: HIn,
    host_stdout: HOut,
) -> Result<IoPump>
where
    HIn: Read + Send + 'static,
    HOut: Write + Send + 'static,
{
    let pty_reader = session.master.try_clone_reader().map_err(Error::Pty)?;
    let pty_writer = session.master.take_writer().map_err(Error::Pty)?;
    let trigger_input = shared_input_pump();
    let host_stdin = InputDetector::new(host_stdin, trigger_input.clone());
    let host_stdout = OutputArbiter::new(host_stdout, trigger_input);

    let host_to_child = spawn_forwarder(Direction::HostToChild, host_stdin, pty_writer);
    let child_to_host = spawn_forwarder(Direction::ChildToHost, pty_reader, host_stdout);

    Ok(IoPump {
        host_to_child,
        child_to_host,
    })
}

// Unix-only real-spawn integration smoke. Exercises
// `start_io_pump` against a real `/bin/echo` child: empty
// host stdin drains the host->child forwarder via ReaderEof,
// while the child->host forwarder captures "hi" into a
// shared sink and converges on the master reader's EOF when
// `echo` exits.
//
// Mirrors the bounded-deadline pattern from
// `tests/pty_smoke.rs` and `src/pty/spawn.rs::real_spawn`
// (constants duplicated locally per repo convention --
// rubber-duck-filtered finding #6 from PR 2).
#[cfg(all(test, unix))]
mod real_pump {
    use super::*;
    use crate::pty::spawn::spawn_child;
    use portable_pty::PtySize;
    use std::ffi::{OsStr, OsString};
    use std::io::{self, Cursor};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::pty::forward::ForwarderExit;

    const JOIN_BUDGET: Duration = Duration::from_secs(5);
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

    /// Shared `Write` sink so the integration test can inspect
    /// what the `child_to_host` forwarder produced after the
    /// thread joins.
    #[derive(Clone, Default)]
    struct SharedSink {
        inner: Arc<Mutex<Vec<u8>>>,
    }
    impl SharedSink {
        fn snapshot(&self) -> Vec<u8> {
            self.inner.lock().unwrap().clone()
        }
    }
    impl io::Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn join_within<T: Send + 'static>(
        handle: std::thread::JoinHandle<T>,
        budget: Duration,
        label: &str,
    ) -> T {
        let deadline = Instant::now() + budget;
        loop {
            if handle.is_finished() {
                return handle.join().unwrap_or_else(|_| panic!("{label} panicked"));
            }
            if Instant::now() >= deadline {
                panic!("{label} did not finish within {budget:?}");
            }
            thread::sleep(WAIT_POLL);
        }
    }

    #[test]
    fn pump_round_trips_echo_output_with_empty_stdin() {
        let mut session = spawn_child(
            OsStr::new("/bin/echo"),
            &[OsString::from("hi")],
            pty_size_80x24(),
        )
        .expect("spawn /bin/echo");

        let mut killer = session.child.clone_killer();
        let sink = SharedSink::default();
        let host_stdin = Cursor::new(Vec::<u8>::new()); // immediate EOF

        let pump = start_io_pump(&mut session, host_stdin, sink.clone()).expect("start_io_pump");

        // Bounded child wait: if echo regresses, kill it so
        // the master reader unblocks and the child_to_host
        // forwarder can return.
        let wait_deadline = Instant::now() + WAIT_BUDGET;
        let status = loop {
            match session.child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if Instant::now() >= wait_deadline {
                        let _ = killer.kill();
                        panic!("child did not exit within {WAIT_BUDGET:?}");
                    }
                    thread::sleep(WAIT_POLL);
                }
                Err(e) => {
                    let _ = killer.kill();
                    panic!("child wait failed: {e}");
                }
            }
        };
        // Drop the master so any lingering reader sees EOF.
        // `start_io_pump` consumed the writer; the cloned
        // reader inside `child_to_host` releases when the
        // master half is dropped.
        drop(session);

        let host_to_child_exit = join_within(pump.host_to_child.join, JOIN_BUDGET, "host_to_child");
        let child_to_host_exit = join_within(pump.child_to_host.join, JOIN_BUDGET, "child_to_host");

        assert!(status.success(), "child exited non-zero: {status:?}");

        // Empty cursor -> immediate ReaderEof with zero bytes.
        let h2c = host_to_child_exit.expect("host_to_child returned io::Error");
        assert_eq!(h2c, ForwarderExit::ReaderEof { bytes: 0 });

        // child_to_host can converge as either ReaderEof
        // (clean cat exit -> master EOF) or WriterClosed
        // (sink dropped its handle first); either is correct
        // for a passive PR-3 forwarder.
        let c2h = child_to_host_exit.expect("child_to_host returned io::Error");
        assert!(
            matches!(
                c2h,
                ForwarderExit::ReaderEof { .. } | ForwarderExit::WriterClosed { .. }
            ),
            "unexpected child_to_host exit: {c2h:?}"
        );

        let captured = String::from_utf8_lossy(&sink.snapshot()).into_owned();
        assert!(
            captured.contains("hi"),
            "expected 'hi' in captured output, got: {captured:?}"
        );
    }

    // Issue #35 specifies that host stdin EOF must close the
    // child's PTY so the wrapped command can exit. /bin/echo
    // doesn't read stdin and therefore can't validate that
    // semantics; /bin/cat does -- it stays alive until its
    // PTY input EOFs. This test pins the host->child EOF
    // propagation path end-to-end. We deliberately do NOT
    // assert on captured bytes (PTY line discipline echoes
    // input, making byte-level comparisons fragile); the
    // contract under test is "host stdin Cursor drains ->
    // host_to_child returns ReaderEof -> writer drops ->
    // child observes EOF -> child exits cleanly within
    // budget".
    #[test]
    fn pump_host_to_child_eof_makes_cat_exit() {
        let mut session =
            spawn_child(OsStr::new("/bin/cat"), &[], pty_size_80x24()).expect("spawn /bin/cat");

        let mut killer = session.child.clone_killer();
        let sink = SharedSink::default();
        // Finite payload: cursor drains -> host_to_child EOFs
        // -> writer drops -> cat sees EOT on slave -> exits.
        let host_stdin = Cursor::new(b"hello\n".to_vec());

        let pump = start_io_pump(&mut session, host_stdin, sink.clone()).expect("start_io_pump");

        let wait_deadline = Instant::now() + WAIT_BUDGET;
        let status = loop {
            match session.child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if Instant::now() >= wait_deadline {
                        let _ = killer.kill();
                        panic!(
                            "cat did not exit on host stdin EOF within {WAIT_BUDGET:?}; \
                             host->child EOF propagation likely regressed"
                        );
                    }
                    thread::sleep(WAIT_POLL);
                }
                Err(e) => {
                    let _ = killer.kill();
                    panic!("child wait failed: {e}");
                }
            }
        };
        drop(session);

        let h2c = join_within(pump.host_to_child.join, JOIN_BUDGET, "host_to_child")
            .expect("host_to_child returned io::Error");
        let c2h = join_within(pump.child_to_host.join, JOIN_BUDGET, "child_to_host")
            .expect("child_to_host returned io::Error");

        assert!(status.success(), "cat exited non-zero: {status:?}");
        assert_eq!(
            h2c,
            ForwarderExit::ReaderEof { bytes: 6 },
            "host_to_child should drain the 6-byte cursor"
        );
        assert!(
            matches!(
                c2h,
                ForwarderExit::ReaderEof { .. } | ForwarderExit::WriterClosed { .. }
            ),
            "unexpected child_to_host exit: {c2h:?}"
        );
        // Sanity: cat must have produced *some* bytes (echoed
        // input under PTY line discipline). Don't pin the
        // exact content -- termios cooked-mode echo is host-
        // dependent.
        assert!(
            !sink.snapshot().is_empty(),
            "cat produced no PTY output -- pump didn't forward anything"
        );
    }
}
