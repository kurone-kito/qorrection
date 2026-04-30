//! Direction-tagged stdio forwarder primitive.
//!
//! [`spawn_forwarder`] owns one half of the wrap session's
//! U-shaped pipe: a thread that reads from `reader`, writes
//! to `writer`, and reports completion via a typed
//! [`ForwarderExit`]. It is deliberately decoupled from the
//! PTY layer (works on any [`Read`] / [`Write`] pair) so that
//! the unit tests can drive the contract in-memory without
//! spawning a real PTY.
//!
//! The wait/drain supervisor that converges both forwarders
//! with the child wait is owned by PR 4 (#33). PR 3 only
//! ships the moving primitive plus its assembly point in
//! [`super::pump`].

use std::io::{self, Read, Write};
use std::thread::{self, JoinHandle};

const BUF_LEN: usize = 8 * 1024;

/// Direction tag carried in [`ForwarderHandle`] so PR 4's
/// supervisor can attribute join failures to the right pipe.
#[allow(dead_code)] // wired into default_body in PR 5 / #26
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    HostToChild,
    ChildToHost,
}

/// Why a forwarder thread stopped.
///
/// Both variants are graceful for PR 3, but the supervisor in
/// PR 4 may want to short-circuit drain on [`Self::WriterClosed`]
/// (the receiving side hung up — e.g. host stdout pager exit,
/// or child closing its stdin after EOF).
#[allow(dead_code)] // wired into default_body in PR 5 / #26
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForwarderExit {
    /// Reader returned `Ok(0)` — natural EOF on the source.
    ReaderEof { bytes: u64 },
    /// Writer raised [`io::ErrorKind::BrokenPipe`] — the
    /// receiving side hung up.
    WriterClosed { bytes: u64 },
}

/// A spawned forwarder thread plus its direction tag.
///
/// The thread keeps running until [`ForwarderExit`] is
/// produced or an unrecoverable [`io::Error`] is returned.
/// PR 4 owns the join policy.
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) struct ForwarderHandle {
    pub(crate) direction: Direction,
    pub(crate) join: JoinHandle<io::Result<ForwarderExit>>,
}

/// Spawn a thread that copies bytes from `reader` into
/// `writer` until EOF or an unrecoverable error.
///
/// See [`ForwarderExit`] for graceful termination tagging and
/// the module-level docs for the I/O policy summary.
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) fn spawn_forwarder<R, W>(
    direction: Direction,
    mut reader: R,
    mut writer: W,
) -> ForwarderHandle
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let join = thread::spawn(move || run_forwarder(&mut reader, &mut writer));
    ForwarderHandle { direction, join }
}

fn run_forwarder<R, W>(reader: &mut R, writer: &mut W) -> io::Result<ForwarderExit>
where
    R: Read,
    W: Write,
{
    let mut buf = [0u8; BUF_LEN];
    let mut bytes: u64 = 0;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => {
                // Final flush is best-effort: we are already
                // terminating cleanly, so a flush error here
                // does not change the outcome.
                let _ = writer.flush();
                return Ok(ForwarderExit::ReaderEof { bytes });
            }
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        match write_all_chunked(writer, &buf[..n]) {
            Ok(()) => bytes += n as u64,
            Err(WriteOutcome::WriterClosed { written }) => {
                bytes += written as u64;
                return Ok(ForwarderExit::WriterClosed { bytes });
            }
            Err(WriteOutcome::Err(e)) => return Err(e),
        }
        // Per-chunk flush so interactive prompts (no trailing
        // newline) reach the consumer immediately. BrokenPipe
        // here mirrors the writer policy.
        match writer.flush() {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                return Ok(ForwarderExit::WriterClosed { bytes });
            }
            Err(e) => return Err(e),
        }
    }
}

enum WriteOutcome {
    WriterClosed { written: usize },
    Err(io::Error),
}

fn write_all_chunked<W: Write>(writer: &mut W, mut data: &[u8]) -> Result<(), WriteOutcome> {
    let mut written = 0usize;
    while !data.is_empty() {
        match writer.write(data) {
            Ok(0) => {
                return Err(WriteOutcome::Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "forwarder writer accepted zero bytes",
                )));
            }
            Ok(n) => {
                written += n;
                data = &data[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                return Err(WriteOutcome::WriterClosed { written });
            }
            Err(e) => return Err(WriteOutcome::Err(e)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Write};
    use std::sync::{Arc, Mutex};

    fn forward_blocking<R, W>(reader: R, writer: W) -> io::Result<ForwarderExit>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let h = spawn_forwarder(Direction::HostToChild, reader, writer);
        h.join.join().expect("forwarder thread panicked")
    }

    // ----- Fakes -----

    /// Reader that yields `Interrupted` once before serving the
    /// underlying `Cursor`.
    struct InterruptOnceReader {
        inner: Cursor<Vec<u8>>,
        tripped: bool,
    }
    impl Read for InterruptOnceReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.tripped {
                self.tripped = true;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "trip"));
            }
            self.inner.read(buf)
        }
    }

    /// Writer that records every accepted byte and `flush` call.
    #[derive(Default, Clone)]
    struct RecordingWriter {
        inner: Arc<Mutex<RecordingState>>,
    }
    #[derive(Default)]
    struct RecordingState {
        bytes: Vec<u8>,
        flush_calls: usize,
    }
    impl RecordingWriter {
        fn snapshot(&self) -> (Vec<u8>, usize) {
            let s = self.inner.lock().unwrap();
            (s.bytes.clone(), s.flush_calls)
        }
    }
    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut s = self.inner.lock().unwrap();
            s.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.inner.lock().unwrap().flush_calls += 1;
            Ok(())
        }
    }

    /// Writer that raises `BrokenPipe` after `cap` bytes accepted.
    struct BrokenPipeAfter {
        accepted: usize,
        cap: usize,
    }
    impl Write for BrokenPipeAfter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.accepted >= self.cap {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "boom"));
            }
            let take = (self.cap - self.accepted).min(buf.len());
            self.accepted += take;
            Ok(take)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Writer that accepts at most `chunk` bytes per call.
    struct PartialWriter {
        inner: Vec<u8>,
        chunk: usize,
    }
    impl Write for PartialWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let take = buf.len().min(self.chunk);
            self.inner.extend_from_slice(&buf[..take]);
            Ok(take)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Writer that always returns `Ok(0)` — exercises the
    /// `WriteZero` guard.
    struct StallWriter;
    impl Write for StallWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Ok(0)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Writer that returns `Interrupted` once before delegating.
    struct InterruptOnceWriter {
        sink: Vec<u8>,
        tripped: bool,
    }
    impl Write for InterruptOnceWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if !self.tripped {
                self.tripped = true;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "trip"));
            }
            self.sink.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Writer that always returns `PermissionDenied`.
    struct DenyWriter;
    impl Write for DenyWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "nope"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // ----- Tests -----

    #[test]
    fn spawn_forwarder_returns_reader_eof_with_byte_count() {
        let payload = b"hello world".to_vec();
        let writer = RecordingWriter::default();
        let probe = writer.clone();
        let exit = forward_blocking(Cursor::new(payload.clone()), writer).unwrap();
        assert_eq!(exit, ForwarderExit::ReaderEof { bytes: 11 });
        let (bytes, _) = probe.snapshot();
        assert_eq!(bytes, payload);
    }

    #[test]
    fn spawn_forwarder_returns_writer_closed_on_broken_pipe() {
        let payload = vec![b'x'; 32];
        let writer = BrokenPipeAfter {
            accepted: 0,
            cap: 5,
        };
        let exit = forward_blocking(Cursor::new(payload), writer).unwrap();
        assert_eq!(exit, ForwarderExit::WriterClosed { bytes: 5 });
    }

    #[test]
    fn spawn_forwarder_handles_partial_writes() {
        let payload = vec![1u8; 17];
        let writer = PartialWriter {
            inner: Vec::new(),
            chunk: 3,
        };
        // Move `writer` into the thread; we cannot probe its
        // sink afterwards, so verify via byte count + EOF
        // tag (writer policy guarantees no drops).
        let exit = forward_blocking(Cursor::new(payload), writer).unwrap();
        assert_eq!(exit, ForwarderExit::ReaderEof { bytes: 17 });
    }

    #[test]
    fn spawn_forwarder_returns_write_zero_when_writer_stalls() {
        let payload = vec![0u8; 4];
        let err = forward_blocking(Cursor::new(payload), StallWriter).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
    }

    #[test]
    fn spawn_forwarder_retries_interrupted_reader() {
        let reader = InterruptOnceReader {
            inner: Cursor::new(b"abc".to_vec()),
            tripped: false,
        };
        let writer = RecordingWriter::default();
        let probe = writer.clone();
        let exit = forward_blocking(reader, writer).unwrap();
        assert_eq!(exit, ForwarderExit::ReaderEof { bytes: 3 });
        assert_eq!(probe.snapshot().0, b"abc");
    }

    #[test]
    fn spawn_forwarder_retries_interrupted_writer() {
        let writer = InterruptOnceWriter {
            sink: Vec::new(),
            tripped: false,
        };
        let exit = forward_blocking(Cursor::new(b"abc".to_vec()), writer).unwrap();
        assert_eq!(exit, ForwarderExit::ReaderEof { bytes: 3 });
    }

    #[test]
    fn spawn_forwarder_propagates_other_writer_errors() {
        let err = forward_blocking(Cursor::new(b"x".to_vec()), DenyWriter).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn spawn_forwarder_flushes_each_chunk() {
        let writer = RecordingWriter::default();
        let probe = writer.clone();
        let _ = forward_blocking(Cursor::new(b"abc".to_vec()), writer).unwrap();
        let (_, flushes) = probe.snapshot();
        // At least one per-chunk flush must precede the EOF
        // path; exact count depends on read chunking but >= 1.
        assert!(flushes >= 1, "expected >=1 flush call, got {flushes}");
    }

    #[test]
    fn forwarder_handle_carries_direction_tag() {
        let h = spawn_forwarder(
            Direction::ChildToHost,
            Cursor::new(Vec::<u8>::new()),
            Vec::<u8>::new(),
        );
        assert_eq!(h.direction, Direction::ChildToHost);
        let _ = h.join.join().unwrap();
    }
}
