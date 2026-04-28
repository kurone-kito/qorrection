//! Signal-driven event pump (Unix self-pipe).
//!
//! qorrection's I/O loop must wake on SIGWINCH (terminal resized
//! → forward new size to the child PTY) and SIGTERM (cooperative
//! shutdown → restore raw mode + reap child). Doing either of
//! those tasks from inside the signal handler is unsafe because
//! they call non-async-signal-safe libc functions. The
//! self-pipe trick translates both signals into ordinary
//! readable bytes that the I/O loop can poll alongside its real
//! file descriptors.
//!
//! Protocol (locked v0.1, see plan §6 D-SIGEVENTS):
//!
//! - SIGWINCH → handler writes one byte: [`EVT_WINCH`] (`b'W'`).
//! - SIGTERM  → handler writes one byte: [`EVT_TERM`] (`b'T'`).
//!
//! The handler does exactly one async-signal-safe `write(2)`
//! and ignores any error (including `EAGAIN` from a full pipe --
//! a pending wake byte is already in flight, which is what we
//! wanted).
//!
//! This module is gated to `cfg(unix)`. Windows uses
//! crossterm-polled events from a 250 ms timer instead (Phase
//! E, D-RESIZE).

#![cfg(unix)]

use std::io;
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crate::Result;

/// Event byte emitted on SIGWINCH (terminal resize).
pub const EVT_WINCH: u8 = b'W';
/// Event byte emitted on SIGTERM (cooperative shutdown).
pub const EVT_TERM: u8 = b'T';

/// Decoded event from the self-pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Resize,
    Shutdown,
}

/// Map a single protocol byte to its [`Event`]. Unknown bytes
/// return `None` so callers can ignore them defensively.
pub fn decode(byte: u8) -> Option<Event> {
    match byte {
        EVT_WINCH => Some(Event::Resize),
        EVT_TERM => Some(Event::Shutdown),
        _ => None,
    }
}

// Signal handlers are process-global. A second SignalGuard would
// stomp the FD the first one's handler is still using, so we
// enforce single-instance installation at runtime.
static INSTALLED: AtomicBool = AtomicBool::new(false);
// Write end of the self-pipe, exposed to the C signal handler.
// `i32` because libc fds are int; -1 means "no handler armed".
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
// Cached self-pipe FDs from previous guard installations. We
// cannot close them on Drop (see the `Drop` comment for the
// async-signal-safe TOCTOU rationale) and we cannot grow them
// either: a long-lived embedding host that installs/drops the
// guard repeatedly would otherwise leak two FDs per cycle until
// `pipe2` fails with `EMFILE`. Reuse the same pair on every
// reinstall instead of opening a fresh pipe each time.
static CACHED_READ_FD: AtomicI32 = AtomicI32::new(-1);
static CACHED_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

/// RAII handle for the installed SIGWINCH/SIGTERM handlers.
///
/// Drop restores the previous handlers but does **not** close
/// the self-pipe FDs (see the comment on `Drop` for the
/// async-signal-safe TOCTOU rationale). The first install opens
/// the pipe and caches both ends in process-global atomics; any
/// subsequent install in the same process reuses that cached
/// pair instead of opening a fresh one, so repeated
/// install/drop cycles in long-lived embedding hosts do not
/// leak two FDs per cycle.
#[derive(Debug)]
pub struct SignalGuard {
    read_fd: RawFd,
    write_fd: RawFd,
    old_winch: libc::sigaction,
    old_term: libc::sigaction,
}

impl SignalGuard {
    /// Install handlers and open the self-pipe.
    ///
    /// Errors if a `SignalGuard` is already live in this
    /// process, or if `pipe2` / `sigaction` fail.
    pub fn install() -> Result<Self> {
        if INSTALLED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "SignalGuard is already installed in this process",
            )
            .into());
        }

        // Reuse the previously-leaked pipe FDs if a SignalGuard
        // was installed and dropped earlier in this process.
        // Otherwise open a fresh pipe and stash the FDs for any
        // future reinstall. `INSTALLED` already serialises
        // these stores -- only one thread reaches this code at
        // a time.
        let cached_r = CACHED_READ_FD.load(Ordering::Acquire);
        let cached_w = CACHED_WRITE_FD.load(Ordering::Acquire);
        let (read_fd, write_fd) = if cached_r >= 0 && cached_w >= 0 {
            (cached_r, cached_w)
        } else {
            // Open the self-pipe with O_NONBLOCK | O_CLOEXEC so the
            // I/O loop can drain it without blocking and forks do
            // not inherit the FDs. macOS lacks `pipe2(2)`, so we
            // emulate it with `pipe` + `fcntl`. The CLOEXEC race
            // window is acceptable: qorrection does not fork between
            // here and the fcntl calls.
            let pair = match open_self_pipe() {
                Ok(p) => p,
                Err(e) => {
                    INSTALLED.store(false, Ordering::Release);
                    return Err(e.into());
                }
            };
            CACHED_READ_FD.store(pair.0, Ordering::Release);
            CACHED_WRITE_FD.store(pair.1, Ordering::Release);
            pair
        };
        WRITE_FD.store(write_fd, Ordering::Release);

        let old_winch = match install_handler(libc::SIGWINCH) {
            Ok(prev) => prev,
            Err(e) => {
                WRITE_FD.store(-1, Ordering::Release);
                cleanup_pipe(read_fd, write_fd);
                INSTALLED.store(false, Ordering::Release);
                return Err(e.into());
            }
        };
        let old_term = match install_handler(libc::SIGTERM) {
            Ok(prev) => prev,
            Err(e) => {
                // SIGWINCH is already installed at this point and a
                // handler invocation may be in flight on another
                // thread, holding our `WRITE_FD` value. Closing the
                // pipe here would race with that write -- exactly
                // the TOCTOU we avoid in `Drop`. Block SIGWINCH on
                // this thread to silence further deliveries here,
                // restore the previous SIGWINCH handler so no new
                // invocations enter our code, then zero `WRITE_FD`
                // and leave the pipe open. The FDs remain in the
                // process-global cache so the next `install` call
                // reuses them rather than opening another pipe.
                let _mask = BlockedSignals::block(&[libc::SIGWINCH]);
                // SAFETY: `old_winch` is the value libc handed us
                // from the matching sigaction call above.
                unsafe {
                    libc::sigaction(libc::SIGWINCH, &old_winch, std::ptr::null_mut());
                }
                WRITE_FD.store(-1, Ordering::Release);
                let _ = (read_fd, write_fd); // intentionally cached, not leaked
                INSTALLED.store(false, Ordering::Release);
                return Err(e.into());
            }
        };

        Ok(Self {
            read_fd,
            write_fd,
            old_winch,
            old_term,
        })
    }

    /// File descriptor to add to the I/O loop's `poll`/`epoll`
    /// set. Becomes readable whenever a signal has fired.
    pub fn read_fd(&self) -> RawFd {
        self.read_fd
    }

    /// Drain every pending event byte from the self-pipe and
    /// decode them. Returns an empty vec if no signals are
    /// pending (`EAGAIN`).
    ///
    /// Unknown bytes are silently dropped -- the protocol is
    /// closed in v0.1, so any unknown byte is by definition a
    /// future-version artifact we should not crash on.
    pub fn drain(&self) -> io::Result<Vec<Event>> {
        let mut events = Vec::new();
        let mut buf = [0u8; 64];
        loop {
            // SAFETY: read into a stack buffer of known length.
            let n = unsafe {
                libc::read(
                    self.read_fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n > 0 {
                // libc::read returns ssize_t; the n > 0 branch above
                // proves the value fits in usize without truncation.
                #[allow(clippy::cast_sign_loss)]
                let n = n as usize;
                events.extend(buf[..n].iter().filter_map(|b| decode(*b)));
                if n < buf.len() {
                    return Ok(events);
                }
                // Loop again -- there may be more.
            } else if n == 0 {
                return Ok(events);
            } else {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock
                    || err.kind() == io::ErrorKind::Interrupted
                {
                    return Ok(events);
                }
                return Err(err);
            }
        }
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        // Closing the self-pipe here would race with any signal
        // handler invocation already in flight on another thread:
        // it could have read the still-valid `WRITE_FD` value, then
        // be preempted, then resume after we close the FD and try
        // to write to a closed-or-recycled descriptor. There is no
        // portable async-signal-safe way to wait for in-flight
        // handlers to finish, so we close the window the only way
        // POSIX allows: restore the previous handlers FIRST (no
        // new invocations can enter our code after `sigaction`
        // returns), zero `WRITE_FD` so the still-running handler
        // skips its write on the next load, and leave both ends
        // of the self-pipe open. Any handler still mid-flight just
        // writes to a still-open fd -- harmless. The two FDs stay
        // cached in `CACHED_READ_FD` / `CACHED_WRITE_FD` and are
        // reused by the next `install` call, so repeated
        // install/drop cycles do not exhaust the process FD table.
        //
        // The thread-local mask still helps: it silences any
        // signal that arrives at this thread while we re-install
        // the previous handlers, which keeps the rollback path
        // out of any signal disposition surprise.
        let _mask = BlockedSignals::block(&[libc::SIGWINCH, libc::SIGTERM]);
        // SAFETY: `old_winch`/`old_term` were obtained from the
        // matching sigaction calls in `install`.
        unsafe {
            libc::sigaction(libc::SIGWINCH, &self.old_winch, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &self.old_term, std::ptr::null_mut());
        }
        WRITE_FD.store(-1, Ordering::Release);
        // FDs deliberately stay open and remain cached for any
        // future `install` (see comment above and `install`).
        let _ = (self.read_fd, self.write_fd);
        INSTALLED.store(false, Ordering::Release);
    }
}

/// RAII helper: block the listed signals on the current thread
/// for the duration of `self`. Used during install rollback and
/// drop so an in-flight signal cannot land on a half-torn-down
/// self-pipe.
struct BlockedSignals {
    prev: libc::sigset_t,
}

impl BlockedSignals {
    fn block(signals: &[libc::c_int]) -> Self {
        // SAFETY: zero-init sigset_t is valid; sigemptyset
        // initializes it; sigaddset writes a single bit.
        let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
        let mut prev: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut set);
            for sig in signals {
                libc::sigaddset(&mut set, *sig);
            }
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut prev);
        }
        Self { prev }
    }
}

impl Drop for BlockedSignals {
    fn drop(&mut self) {
        // SAFETY: `prev` is what pthread_sigmask gave us.
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.prev, std::ptr::null_mut());
        }
    }
}

fn open_self_pipe() -> io::Result<(RawFd, RawFd)> {
    open_self_pipe_impl()
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd"))]
fn open_self_pipe_impl() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe2 writes exactly two int FDs into the
    // provided buffer when it returns 0; we check the rc.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd")))]
fn open_self_pipe_impl() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe writes exactly two int FDs into the
    // provided buffer when it returns 0; we check the rc.
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        // SAFETY: fcntl on raw FDs we just opened ourselves.
        unsafe {
            if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1
                || libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) == -1
            {
                let err = io::Error::last_os_error();
                libc::close(fds[0]);
                libc::close(fds[1]);
                return Err(err);
            }
        }
    }
    Ok((fds[0], fds[1]))
}

fn install_handler(signum: libc::c_int) -> io::Result<libc::sigaction> {
    // SAFETY: zero-initialized sigaction is valid; we fill it
    // before passing to sigaction.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    // SAFETY: sigemptyset writes into a valid sigset_t pointer.
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }
    let mut prev: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers point at locals of the right type.
    let rc = unsafe { libc::sigaction(signum, &action, &mut prev) };
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(prev)
    }
}

fn cleanup_pipe(read_fd: RawFd, write_fd: RawFd) {
    // SAFETY: close on raw FDs we opened ourselves; ignore
    // errors because there is nothing useful to do with them.
    unsafe {
        libc::close(read_fd);
        libc::close(write_fd);
    }
}

extern "C" fn handler(sig: libc::c_int) {
    let fd = WRITE_FD.load(Ordering::Acquire);
    if fd < 0 {
        return;
    }
    let byte: u8 = match sig {
        libc::SIGWINCH => EVT_WINCH,
        libc::SIGTERM => EVT_TERM,
        _ => return,
    };
    // SAFETY: write(2) is async-signal-safe (POSIX.1-2017). We
    // write exactly one byte from a stack location; failure
    // (including EAGAIN on a full pipe) is intentionally
    // ignored -- a wake byte is already pending in that case.
    unsafe {
        libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Singleton install means tests cannot run in parallel.
    // One mutex serializes any test that touches the guard.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn decode_known_bytes() {
        assert_eq!(decode(EVT_WINCH), Some(Event::Resize));
        assert_eq!(decode(EVT_TERM), Some(Event::Shutdown));
    }

    #[test]
    fn decode_unknown_byte_is_none() {
        assert!(decode(b'?').is_none());
        assert!(decode(0).is_none());
    }

    #[test]
    fn install_and_drain_winch() {
        let _serial = SERIAL.lock().unwrap();
        let guard = SignalGuard::install().expect("install");
        // SAFETY: kill on our own pid with SIGWINCH (default
        // action: ignore) is safe even without a handler.
        let rc = unsafe { libc::kill(libc::getpid(), libc::SIGWINCH) };
        assert_eq!(rc, 0);
        // Give the handler a moment; the byte should already be
        // there because signal delivery on self-kill is
        // synchronous in practice on Linux/macOS, but loop
        // briefly to be safe.
        let mut events = Vec::new();
        for _ in 0..100 {
            events.extend(guard.drain().unwrap());
            if !events.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(events, vec![Event::Resize]);
    }

    #[test]
    fn double_install_is_rejected() {
        let _serial = SERIAL.lock().unwrap();
        let _g = SignalGuard::install().expect("first install");
        let err = SignalGuard::install().expect_err("second install must fail");
        assert!(err.to_string().contains("already installed"), "{err}");
    }

    #[test]
    fn drop_restores_previous_handler() {
        let _serial = SERIAL.lock().unwrap();
        // Install + drop, then verify SIGWINCH no longer writes
        // to a (now-closed) pipe by re-installing and confirming
        // it succeeds -- a stale handler would be using the old
        // FD and we'd never know, but at least the singleton
        // bookkeeping must be reset.
        {
            let _g = SignalGuard::install().expect("install");
        }
        let _g2 = SignalGuard::install().expect("re-install after drop");
    }

    #[test]
    fn reinstall_reuses_cached_self_pipe_fds() {
        // Regression: Drop must not close the self-pipe (TOCTOU
        // with in-flight handlers), but the FDs also must not
        // accumulate across install/drop cycles. The first
        // install opens the pipe and caches both ends; every
        // subsequent install in this process must reuse those
        // exact descriptors.
        let _serial = SERIAL.lock().unwrap();
        let (first_r, first_w) = {
            let g = SignalGuard::install().expect("first install");
            (g.read_fd, g.write_fd)
        };
        for _ in 0..5 {
            let g = SignalGuard::install().expect("reinstall");
            assert_eq!(g.read_fd, first_r, "read_fd must be reused");
            assert_eq!(g.write_fd, first_w, "write_fd must be reused");
        }
    }
}
