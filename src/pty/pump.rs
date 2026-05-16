//! Assemble the host↔child I/O pump over a [`SpawnedSession`].
//!
//! [`start_io_pump`] consumes the master writer and clones the
//! master reader from a live PTY spawn, then hands them to
//! [`super::forward::spawn_forwarder`] in both directions. The
//! returned [`IoPump`] is a passive bundle of two
//! [`super::forward::ForwarderHandle`]s; the wait/drain
//! supervisor that converges them with the child wait is owned
//! by PR 4 (#33).

use std::io::{self, Read, Write};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::RawFd;

use crate::anim::render::{draw_frame, render_plan, FRAME_DELAY};
use crate::pty::forward::{
    spawn_cancellable_forwarder, spawn_forwarder, CancelHandle, CancellableReader, Direction,
    ForwarderHandle,
};
use crate::pty::spawn::SpawnedSession;
use crate::trigger::{
    input::{shared_input_pump, shared_render_progress, InputInterceptor, SharedRenderProgress},
    output::OutputArbiter,
    parser::Outcome,
};
use crate::{Error, Result};

// Hosted macOS runners can spend roughly one extra 50 ms tick
// per frame on draw/flush scheduling beyond the nominal hold
// delay, so budget each remaining frame as "draw + hold" rather
// than "hold only" when the supervisor waits for an in-flight
// animation to restore the primary screen.
const RENDER_FRAME_BUDGET_MULTIPLIER: u32 = 2;
// Even after the last frame draw returns, give the renderer a
// little extra wall-clock slack to run the terminal-guard drop
// and flush the leave-alt-screen sequence on contended CI hosts.
const RENDER_JOIN_SLACK: Duration = Duration::from_secs(2);

/// Owning bundle of the host↔child forwarder threads.
///
/// Direction tags are preserved so PR 4's supervisor can
/// attribute join failures (and decide drain ordering) without
/// relying on field position.
pub(crate) struct IoPump {
    pub(crate) host_to_child: ForwarderHandle,
    pub(crate) child_to_host: ForwarderHandle,
    pub(crate) host_to_child_render_progress: Option<SharedRenderProgress>,
}

impl IoPump {
    pub(crate) fn into_parts(
        self,
    ) -> (
        ForwarderHandle,
        ForwarderHandle,
        Option<SharedRenderProgress>,
    ) {
        (
            self.host_to_child,
            self.child_to_host,
            self.host_to_child_render_progress,
        )
    }

    pub(crate) fn host_to_child_post_exit_budget(
        render_progress: Option<&SharedRenderProgress>,
        base: Duration,
    ) -> Duration {
        let Some(render_progress) = render_progress else {
            return base;
        };
        let frames_remaining = render_progress.load(Ordering::SeqCst);
        if frames_remaining == 0 {
            return base;
        }

        let frame_budget_units =
            frames_remaining.saturating_mul(RENDER_FRAME_BUDGET_MULTIPLIER as usize);
        let frame_budget_units = u32::try_from(frame_budget_units).unwrap_or(u32::MAX);
        let render_budget = FRAME_DELAY
            .checked_mul(frame_budget_units)
            .unwrap_or(Duration::from_secs(60))
            + RENDER_JOIN_SLACK;
        base.max(render_budget)
    }
}

enum HostToChildWriter<W> {
    Armed(InputInterceptor<W>),
    Passthrough(W),
}

impl<W> Write for HostToChildWriter<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Armed(writer) => writer.write(buf),
            Self::Passthrough(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Armed(writer) => writer.flush(),
            Self::Passthrough(writer) => writer.flush(),
        }
    }
}

enum ChildToHostWriter<W> {
    Armed(OutputArbiter<W>),
    Passthrough(W),
}

impl<W> Write for ChildToHostWriter<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Armed(writer) => writer.write(buf),
            Self::Passthrough(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Armed(writer) => writer.flush(),
            Self::Passthrough(writer) => writer.flush(),
        }
    }
}

struct SharedWriter<W> {
    inner: Arc<Mutex<W>>,
}

impl<W> Clone for SharedWriter<W> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<W> SharedWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, W> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "shared host stdout mutex was poisoned; recovering writer"
                );
                err.into_inner()
            }
        }
    }
}

impl<W> Write for SharedWriter<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.lock();
        guard.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self.lock();
        guard.flush()
    }
}

struct LockedPresentation<'a, W: Write> {
    out: MutexGuard<'a, W>,
}

impl<'a, W> LockedPresentation<'a, W>
where
    W: Write,
{
    fn acquire(mut out: MutexGuard<'a, W>) -> Result<Self> {
        crossterm::execute!(&mut *out, crossterm::terminal::EnterAlternateScreen)?;
        if let Err(err) = crossterm::execute!(&mut *out, crossterm::cursor::Hide) {
            let _ = crossterm::execute!(&mut *out, crossterm::terminal::LeaveAlternateScreen);
            return Err(err.into());
        }
        Ok(Self { out })
    }

    fn writer(&mut self) -> &mut W {
        &mut self.out
    }
}

impl<W> Drop for LockedPresentation<'_, W>
where
    W: Write,
{
    fn drop(&mut self) {
        let _ = crossterm::execute!(&mut *self.out, crossterm::cursor::Show);
        let _ = crossterm::execute!(&mut *self.out, crossterm::terminal::LeaveAlternateScreen);
    }
}

fn render_cols() -> u16 {
    match crossterm::terminal::size() {
        Ok((cols, _)) if cols > 0 => cols,
        _ => 80,
    }
}

struct RenderProgressGuard<'a> {
    frames_remaining: &'a AtomicUsize,
}

impl Drop for RenderProgressGuard<'_> {
    fn drop(&mut self) {
        self.frames_remaining.store(0, Ordering::SeqCst);
    }
}

fn render_animation<W>(
    host_stdout: &SharedWriter<W>,
    outcome: Outcome,
    frames_remaining: &AtomicUsize,
) -> io::Result<()>
where
    W: Write,
{
    let Some(plan) = render_plan(outcome, render_cols()) else {
        return Ok(());
    };
    if plan.frames.is_empty() {
        return Ok(());
    }

    frames_remaining.store(plan.frames.len(), Ordering::SeqCst);
    let _render_progress = RenderProgressGuard { frames_remaining };
    let mut presentation =
        LockedPresentation::acquire(host_stdout.lock()).map_err(io::Error::other)?;
    for frame in &plan.frames {
        draw_frame(presentation.writer(), frame).map_err(io::Error::other)?;
        std::thread::sleep(FRAME_DELAY);
        frames_remaining.fetch_sub(1, Ordering::SeqCst);
    }
    Ok(())
}

struct TriggerWiring<PtyW, HOut> {
    host_to_child: HostToChildWriter<PtyW>,
    child_to_host: ChildToHostWriter<SharedWriter<HOut>>,
    input: Option<crate::trigger::input::SharedInputPump>,
    render_progress: Option<SharedRenderProgress>,
}

fn wire_trigger_io<PtyW, HOut>(
    armed: bool,
    pty_writer: PtyW,
    host_stdout: HOut,
) -> TriggerWiring<PtyW, HOut>
where
    PtyW: Write,
    HOut: Write + Send + 'static,
{
    let shared_stdout = SharedWriter::new(host_stdout);
    if armed {
        let input = shared_input_pump();
        let render_stdout = shared_stdout.clone();
        let render_progress = shared_render_progress();
        let callback_render_progress = render_progress.clone();
        TriggerWiring {
            host_to_child: HostToChildWriter::Armed(InputInterceptor::new(
                pty_writer,
                input.clone(),
                move |outcome| {
                    render_animation(&render_stdout, outcome, callback_render_progress.as_ref())
                },
            )),
            child_to_host: ChildToHostWriter::Armed(OutputArbiter::new(
                shared_stdout,
                input.clone(),
            )),
            input: Some(input),
            render_progress: Some(render_progress),
        }
    } else {
        TriggerWiring {
            host_to_child: HostToChildWriter::Passthrough(pty_writer),
            child_to_host: ChildToHostWriter::Passthrough(shared_stdout),
            input: None,
            render_progress: None,
        }
    }
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
fn start_io_pump_with_reader<HIn, HOut>(
    session: &mut SpawnedSession,
    host_stdin: CancellableReader<HIn>,
    host_stdout: HOut,
    armed: bool,
    host_cancel_wakes_read: bool,
) -> Result<IoPump>
where
    HIn: Read + Send + 'static,
    HOut: Write + Send + 'static,
{
    let pty_reader = session.master.try_clone_reader().map_err(Error::Pty)?;
    let pty_writer = session.master.take_writer().map_err(Error::Pty)?;
    let TriggerWiring {
        host_to_child: pty_writer,
        child_to_host: host_stdout,
        input: _input,
        render_progress,
    } = wire_trigger_io(armed, pty_writer, host_stdout);

    let host_to_child = spawn_cancellable_forwarder(
        Direction::HostToChild,
        host_stdin,
        pty_writer,
        host_cancel_wakes_read,
    );
    let child_to_host = spawn_forwarder(Direction::ChildToHost, pty_reader, host_stdout);

    Ok(IoPump {
        host_to_child,
        child_to_host,
        host_to_child_render_progress: render_progress,
    })
}

#[cfg(any(not(unix), test))]
pub(crate) fn start_io_pump<HIn, HOut>(
    session: &mut SpawnedSession,
    host_stdin: HIn,
    host_stdout: HOut,
    armed: bool,
) -> Result<IoPump>
where
    HIn: Read + Send + 'static,
    HOut: Write + Send + 'static,
{
    let host_cancel = CancelHandle::new();
    start_io_pump_with_reader(
        session,
        CancellableReader::new(host_stdin, host_cancel),
        host_stdout,
        armed,
        false,
    )
}

#[cfg(unix)]
pub(crate) fn start_io_pump_pollable<HIn, HOut>(
    session: &mut SpawnedSession,
    host_stdin: HIn,
    host_stdout: HOut,
    armed: bool,
    host_stdin_fd: RawFd,
) -> Result<IoPump>
where
    HIn: Read + Send + 'static,
    HOut: Write + Send + 'static,
{
    let host_cancel = CancelHandle::new();
    start_io_pump_with_reader(
        session,
        CancellableReader::with_poll_fd(host_stdin, host_cancel, host_stdin_fd),
        host_stdout,
        armed,
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::parser::Outcome;
    use std::{sync::mpsc, thread, time::Duration};

    const ENTER_ALT: &[u8] = b"\x1b[?1049h";

    #[test]
    fn armed_wiring_shares_trigger_state_with_input_detector() {
        let mut wiring = wire_trigger_io(true, Vec::new(), Vec::new());
        assert!(matches!(wiring.host_to_child, HostToChildWriter::Armed(_)));
        assert!(matches!(wiring.child_to_host, ChildToHostWriter::Armed(_)));

        let input = wiring
            .input
            .as_ref()
            .expect("armed wiring should allocate shared trigger state")
            .clone();

        wiring.host_to_child.write_all(b":").unwrap();

        let mut guard = input.lock().unwrap();
        assert_eq!(guard.feed_input_byte(b"q"[0]).outcome(), Outcome::None);
        assert_eq!(guard.feed_input_byte(b"\n"[0]).outcome(), Outcome::Q);
    }

    #[test]
    fn armed_wiring_shares_trigger_state_with_output_arbiter() {
        let mut wiring = wire_trigger_io(true, Vec::new(), Vec::new());
        let input = wiring
            .input
            .as_ref()
            .expect("armed wiring should allocate shared trigger state")
            .clone();

        wiring.child_to_host.write_all(ENTER_ALT).unwrap();

        assert!(
            input.lock().unwrap().is_alt_screen(),
            "armed output path should keep alt-screen state in sync"
        );
    }

    #[test]
    fn armed_wiring_fires_animation_instead_of_forwarding_trigger_bytes() {
        let mut wiring = wire_trigger_io(true, Vec::new(), Vec::new());

        wiring.host_to_child.write_all(b":q\n").unwrap();

        let HostToChildWriter::Armed(host) = &wiring.host_to_child else {
            panic!("armed host path should use the trigger interceptor");
        };
        assert_eq!(
            host.inner().as_slice(),
            b"",
            "fired trigger bytes must not reach the child PTY"
        );

        let ChildToHostWriter::Armed(child) = &wiring.child_to_host else {
            panic!("armed child path should use the output arbiter");
        };
        let rendered = child.inner().lock();
        let text = String::from_utf8_lossy(rendered.as_slice());
        assert!(
            text.contains("Fi-Fo") || text.contains("[QQ]") || text.contains("QUEUE"),
            "expected renderer output on host stdout, got {text:?}"
        );
    }

    #[test]
    fn disarmed_wiring_bypasses_trigger_state_entirely() {
        let mut wiring = wire_trigger_io(false, Vec::new(), Vec::new());
        assert!(matches!(
            wiring.host_to_child,
            HostToChildWriter::Passthrough(_)
        ));
        assert!(matches!(
            wiring.child_to_host,
            ChildToHostWriter::Passthrough(_)
        ));
        assert!(
            wiring.input.is_none(),
            "disarmed wiring must not allocate trigger state"
        );

        wiring.host_to_child.write_all(b":q\n").unwrap();
        wiring.child_to_host.write_all(ENTER_ALT).unwrap();

        let HostToChildWriter::Passthrough(host_bytes) = wiring.host_to_child else {
            panic!("disarmed host path should remain a passthrough writer");
        };
        assert_eq!(host_bytes, b":q\n");

        let ChildToHostWriter::Passthrough(child_bytes) = wiring.child_to_host else {
            panic!("disarmed child path should remain a passthrough writer");
        };
        assert_eq!(child_bytes.lock().as_slice(), ENTER_ALT);
    }

    #[test]
    fn render_progress_budget_uses_frame_overhead_from_zero_base() {
        let progress = Arc::new(AtomicUsize::new(3));

        let budget = IoPump::host_to_child_post_exit_budget(Some(&progress), Duration::ZERO);

        assert_eq!(
            budget,
            FRAME_DELAY
                .checked_mul(3 * RENDER_FRAME_BUDGET_MULTIPLIER)
                .expect("small test multiplier should fit")
                + RENDER_JOIN_SLACK
        );
    }

    #[test]
    fn shared_writer_blocks_other_handles_until_lock_is_released() {
        let shared = SharedWriter::new(Vec::new());
        let locked = shared.lock();
        let mut writer = shared.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let join = thread::spawn(move || {
            started_tx.send(()).unwrap();
            writer.write_all(b"child").unwrap();
            done_tx.send(()).unwrap();
        });

        started_rx.recv().unwrap();
        assert!(
            done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "child output should block while the renderer owns the lock"
        );

        drop(locked);

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("child output should resume once the lock is released");
        join.join().unwrap();

        assert_eq!(shared.lock().as_slice(), b"child");
    }
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

        let pump =
            start_io_pump(&mut session, host_stdin, sink.clone(), true).expect("start_io_pump");

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

        let pump =
            start_io_pump(&mut session, host_stdin, sink.clone(), true).expect("start_io_pump");

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
