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
use crate::{Error, Result};

/// Owning bundle of the host↔child forwarder threads.
///
/// Direction tags are preserved so PR 4's supervisor can
/// attribute join failures (and decide drain ordering) without
/// relying on field position.
#[allow(dead_code)] // wired into default_body in PR 5 / #26
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
#[allow(dead_code)] // wired into default_body in PR 5 / #26
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

    let host_to_child = spawn_forwarder(Direction::HostToChild, host_stdin, pty_writer);
    let child_to_host = spawn_forwarder(Direction::ChildToHost, pty_reader, host_stdout);

    Ok(IoPump {
        host_to_child,
        child_to_host,
    })
}
