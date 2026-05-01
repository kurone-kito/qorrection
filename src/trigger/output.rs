//! Output-side trigger arbiter.
//!
//! `OutputArbiter` is a [`Write`] adapter for the child-to-host
//! pump. It forwards child output unchanged while feeding the
//! accepted bytes into the shared [`super::input::InputPump`] so
//! alternate-screen CSI sequences can disarm input-side trigger
//! parsing.
//!
//! The adapter observes only bytes accepted by the wrapped writer.
//! This keeps partial writes safe: the forwarder will retry the
//! remaining suffix, and those bytes are observed exactly when that
//! retry is accepted.

use std::io::{self, Write};

use super::input::SharedInputPump;

/// Child-output [`Write`] adapter that updates trigger state while
/// preserving byte-for-byte passthrough.
#[derive(Debug)]
pub struct OutputArbiter<W> {
    inner: W,
    input: SharedInputPump,
}

impl<W> OutputArbiter<W> {
    pub fn new(inner: W, input: SharedInputPump) -> Self {
        Self { inner, input }
    }

    #[cfg(test)]
    fn inner(&self) -> &W {
        &self.inner
    }
}

impl<W> Write for OutputArbiter<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        if written > 0 {
            let mut input = self.input.lock().map_err(|_| {
                io::Error::other("trigger input pump mutex poisoned while observing child output")
            })?;
            input.feed_child_output_slice(&buf[..written]);
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::input::shared_input_pump;

    const ENTER_ALT: &[u8] = b"\x1b[?1049h";
    const LEAVE_ALT: &[u8] = b"\x1b[?1049l";

    #[derive(Debug, Default)]
    struct OneByteWriter {
        bytes: Vec<u8>,
    }

    impl Write for OneByteWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            self.bytes.push(buf[0]);
            Ok(1)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct ZeroWriter;

    impl Write for ZeroWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Ok(0)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn forwards_output_unchanged() {
        let input = shared_input_pump();
        let mut arbiter = OutputArbiter::new(Vec::new(), input.clone());

        arbiter.write_all(b"hello").unwrap();

        assert_eq!(arbiter.inner(), b"hello");
        assert!(!input.lock().unwrap().is_alt_screen());
    }

    #[test]
    fn feeds_alt_screen_enter_and_leave() {
        let input = shared_input_pump();
        let mut arbiter = OutputArbiter::new(Vec::new(), input.clone());

        arbiter.write_all(ENTER_ALT).unwrap();
        assert!(input.lock().unwrap().is_alt_screen());

        arbiter.write_all(LEAVE_ALT).unwrap();
        assert!(!input.lock().unwrap().is_alt_screen());
    }

    #[test]
    fn partial_writes_are_observed_once() {
        let input = shared_input_pump();
        let mut arbiter = OutputArbiter::new(OneByteWriter::default(), input.clone());

        arbiter.write_all(ENTER_ALT).unwrap();

        assert_eq!(arbiter.inner().bytes, ENTER_ALT);
        assert!(input.lock().unwrap().is_alt_screen());
    }

    #[test]
    fn unwritten_bytes_are_not_observed() {
        let input = shared_input_pump();
        let mut arbiter = OutputArbiter::new(ZeroWriter, input.clone());

        assert_eq!(arbiter.write(ENTER_ALT).unwrap(), 0);

        assert!(!input.lock().unwrap().is_alt_screen());
    }
}
