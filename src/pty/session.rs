//! Wait + drain supervisor for an [`IoPump`] over a
//! [`SpawnedSession`].
//!
//! ## What this owns
//!
//! [`run_pump_session`] is the convergence point for the wrapped
//! child and the two forwarder threads spun up by
//! [`super::pump::start_io_pump`]. It blocks until the child
//! exits, drains the forwarders, and returns the ExitCode the
//! Wrap path should bubble up to the binary entry point.
//!
//! ## Drain ordering — why a coupled state machine
//!
//! A naive "wait-first, then join forwarders" ordering deadlocks
//! when the child stalls writing to a stalled host stdout: the
//! child cannot exit, the supervisor cannot advance, and both
//! forwarders are blocked. Instead this module runs a coupled
//! poll loop: every tick checks `child.try_wait()` AND probes
//! the two `JoinHandle::is_finished()` flags. If both forwarders
//! converge while the child is still alive (no producer / no
//! consumer) the supervisor escalates [`ChildKiller::kill`] and
//! resumes the wait poll. If one forwarder returns an `io::Error`
//! before the child exits, the supervisor likewise kills the
//! child and surfaces the error as [`Error::Pty`]. After a clean
//! wait, forwarder I/O errors are logged via `tracing::warn!`
//! and *not* propagated — the child's status is authoritative.
//!
//! ## Kill-on-drop guard
//!
//! `portable-pty`'s `Drop for Box<dyn Child>` does NOT wait for
//! or signal the child (verified in
//! [`super::spawn::SpawnedSession`] doc comment). [`KillOnDropGuard`]
//! arms on entry, holds a cloned [`portable_pty::ChildKiller`],
//! and best-effort `kill()`s the child on every unwind path
//! (panic, early `?`, …) until it's disarmed by a successful
//! wait. The guard is unit-tested via `catch_unwind`.
//!
//! ## Detached-thread degraded mode
//!
//! `host_to_child` is the only forwarder that may stay blocked
//! after the child exits — it can be sitting in `read()` on real
//! host stdin with no graceful cancellation primitive. If the
//! join exceeds [`Deadlines::forwarder_join_budget`] we emit a
//! `tracing::warn!`, drop the join handle (detaching the OS
//! thread), and let the parent process termination clean it up.
//! A first-class cancellation API for forwarders is tracked as a
//! follow-up — see issue #89 (`pty: cancellable forwarders for
//! non-EOF host stdin`).

use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use portable_pty::{ChildKiller, ExitStatus};

use crate::pty::exit::map_exit_status;
use crate::pty::forward::{Direction, ForwarderExit, ForwarderHandle};
use crate::pty::pump::IoPump;
use crate::pty::spawn::SpawnedSession;
use crate::{Error, Result};

/// Polling and join time budgets the supervisor honors.
///
/// `child_wait_deadline = None` is the production setting — the
/// loop polls forever, with convergence guaranteed by either
/// (a) the child exits naturally or (b) both forwarders converge
/// and the supervisor escalates `kill()`. A finite deadline is
/// only meaningful in tests that need bounded run time.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) struct Deadlines {
    /// Optional wall-clock deadline on Phase 1 (waiting for the
    /// child). `None` in production (poll forever). Tests pass
    /// a tight value (e.g. 50ms) for bounded runtime.
    pub child_wait_deadline: Option<Duration>,
    /// Maximum time spent joining each forwarder thread after
    /// the child has exited. On timeout the join handle is
    /// dropped and a `warn!` is emitted (detached-thread mode).
    pub forwarder_join_budget: Duration,
    /// Sleep between successive `try_wait` ticks. Keeps the
    /// supervisor from busy-looping; small enough that signal
    /// death is observed promptly.
    pub wait_poll: Duration,
}

impl Deadlines {
    /// Production defaults: poll forever, 5s join budget, 20ms
    /// poll cadence. These match the rest of the PTY layer's
    /// per-file constants (see `pty/spawn.rs::real_spawn`,
    /// `pty/pump.rs::real_pump`).
    #[allow(dead_code)] // wired into default_body in PR 5 / #26
    pub(crate) const fn production() -> Self {
        Self {
            child_wait_deadline: None,
            forwarder_join_budget: Duration::from_secs(5),
            wait_poll: Duration::from_millis(20),
        }
    }
}

/// Trait-level seam for the wrapped child.
///
/// Production binds [`PtyChild`] (a thin wrapper over a
/// `Box<dyn portable_pty::Child + Send + Sync>` + a cloned
/// `Box<dyn ChildKiller + Send + Sync>`). Tests bind in-memory
/// mocks driving the supervisor through every branch without a
/// real PTY.
pub(crate) trait SupervisedChild {
    /// Non-blocking wait. Wraps `portable_pty::Child::try_wait`.
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    /// Blocking wait. Wraps `portable_pty::Child::wait`.
    fn wait(&mut self) -> io::Result<ExitStatus>;
    /// Produce a fresh killer handle. Wraps
    /// `portable_pty::Child::clone_killer`.
    fn clone_killer(&mut self) -> Box<dyn ChildKiller + Send + Sync>;
}

/// Production [`SupervisedChild`] over a `portable-pty` child.
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) struct PtyChild {
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl SupervisedChild for PtyChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }
    fn clone_killer(&mut self) -> Box<dyn ChildKiller + Send + Sync> {
        self.child.clone_killer()
    }
}

/// RAII guard that best-effort `kill()`s the child unless
/// disarmed. Documented invariant: armed until a successful
/// wait returns and consumes a status. See module-level docs.
struct KillOnDropGuard {
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
}

impl KillOnDropGuard {
    fn armed(killer: Box<dyn ChildKiller + Send + Sync>) -> Self {
        Self {
            killer: Some(killer),
        }
    }

    fn disarm(&mut self) {
        self.killer = None;
    }
}

impl Drop for KillOnDropGuard {
    fn drop(&mut self) {
        if let Some(mut k) = self.killer.take() {
            // Best-effort. Errors here can only be diagnostic —
            // we cannot recover from a failed kill in a destructor
            // path. Log via `tracing::warn!` so observability
            // sees it.
            if let Err(e) = k.kill() {
                tracing::warn!(error = %e, "KillOnDropGuard: best-effort kill failed");
            }
        }
    }
}

/// Production entry point. Wraps [`SpawnedSession`] +
/// [`IoPump`] into the trait-seam form and delegates to
/// [`run_pump_session_with`].
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) fn run_pump_session(session: SpawnedSession, pump: IoPump) -> Result<ExitCode> {
    // Bind the master to a named local so it stays alive for
    // the entire supervised session. A wildcard (`master: _`)
    // pattern would drop it immediately at the destructuring
    // point, EOF'ing the slave side and racing the child / the
    // forwarder threads. The named binding extends its lifetime
    // to the end of the function, so the master is dropped only
    // after `run_pump_session_with` returns.
    let SpawnedSession {
        child,
        master: _master,
    } = session;
    let child = PtyChild { child };
    run_pump_session_with(child, pump, Deadlines::production())
}

/// Lifecycle core, parameterised over the [`SupervisedChild`]
/// implementation and the time [`Deadlines`]. Exists so unit
/// tests can drive every branch of the state machine without a
/// real PTY.
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) fn run_pump_session_with<C>(
    mut child: C,
    pump: IoPump,
    deadlines: Deadlines,
) -> Result<ExitCode>
where
    C: SupervisedChild,
{
    let mut guard = KillOnDropGuard::armed(child.clone_killer());

    let mut h2c = Some(pump.host_to_child);
    let mut c2h = Some(pump.child_to_host);
    let mut h2c_result: Option<io::Result<ForwarderExit>> = None;
    let mut c2h_result: Option<io::Result<ForwarderExit>> = None;

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return Err(wrap_io("child try_wait", e)),
        }

        // Harvest finished forwarders so we can inspect their
        // results in subsequent ticks. This is a non-blocking
        // peek: `is_finished()` says the join would not block,
        // and `join()` then returns immediately.
        harvest(&mut h2c, &mut h2c_result);
        harvest(&mut c2h, &mut c2h_result);

        let h2c_errored = matches!(h2c_result, Some(Err(_)));
        let c2h_errored = matches!(c2h_result, Some(Err(_)));
        if h2c_errored || c2h_errored {
            // RD finding 8: forwarder I/O error before the child
            // exits is unrecoverable. Kill, wait, surface.
            escalate_kill(&mut child);
            let _ = child.wait(); // reap, ignore status
                                  // Pull the actual io::Error out for the Pty wrapper.
            let err = h2c_result
                .take()
                .and_then(|r| r.err())
                .or_else(|| c2h_result.take().and_then(|r| r.err()))
                .unwrap_or_else(|| io::Error::other("forwarder failed (no io::Error captured)"));
            return Err(wrap_io("forwarder failed", err));
        }

        if h2c_result.is_some() && c2h_result.is_some() {
            // Both forwarders converged while the child is still
            // alive → no producer, no consumer. Escalate kill
            // and block on `wait()`.
            escalate_kill(&mut child);
            match child.wait() {
                Ok(s) => break s,
                Err(e) => return Err(wrap_io("child wait after kill", e)),
            }
        }

        if let Some(d) = deadlines.child_wait_deadline {
            if start.elapsed() >= d {
                // Test-only convergence path: bail by killing
                // and waiting. Production passes None.
                escalate_kill(&mut child);
                match child.wait() {
                    Ok(s) => break s,
                    Err(e) => return Err(wrap_io("child wait after deadline kill", e)),
                }
            }
        }

        std::thread::sleep(deadlines.wait_poll);
    };

    // Wait consumed a status → guard's job is done.
    guard.disarm();

    // Phase 2: drain remaining forwarders within budget.
    if let Some(h) = h2c.take() {
        h2c_result = Some(join_with_budget(h, deadlines.forwarder_join_budget));
    }
    if let Some(h) = c2h.take() {
        c2h_result = Some(join_with_budget(h, deadlines.forwarder_join_budget));
    }
    log_forwarder_outcome(Direction::HostToChild, h2c_result);
    log_forwarder_outcome(Direction::ChildToHost, c2h_result);

    map_exit_status(status)
}

fn wrap_io(context: &'static str, e: io::Error) -> Error {
    Error::Pty(anyhow::Error::new(e).context(context))
}

fn escalate_kill<C: SupervisedChild>(child: &mut C) {
    let mut k = child.clone_killer();
    if let Err(e) = k.kill() {
        tracing::warn!(error = %e, "supervisor: best-effort kill failed");
    }
}

/// Non-blocking peek + extract. If the handle is finished, join
/// it (returns instantly) and stash the result. Otherwise leave
/// the handle in place for the next tick.
fn harvest(slot: &mut Option<ForwarderHandle>, result: &mut Option<io::Result<ForwarderExit>>) {
    let take_now = slot.as_ref().is_some_and(|h| h.join.is_finished());
    if !take_now {
        return;
    }
    let handle = slot.take().expect("checked above");
    *result = Some(extract_join(handle));
}

/// Join a finished (or about-to-finish) forwarder, flattening
/// the panic case to an `io::Error` for uniform handling.
fn extract_join(handle: ForwarderHandle) -> io::Result<ForwarderExit> {
    match handle.join.join() {
        Ok(r) => r,
        Err(_) => Err(io::Error::other("forwarder thread panicked")),
    }
}

/// Join with a wall-clock budget. On timeout drop the handle
/// (detaching the underlying OS thread) and synthesize a
/// `TimedOut` error so the caller can log it.
fn join_with_budget(handle: ForwarderHandle, budget: Duration) -> io::Result<ForwarderExit> {
    let deadline = Instant::now() + budget;
    let direction = handle.direction;
    // Poll-based wait so we never block past the deadline.
    while Instant::now() < deadline {
        if handle.join.is_finished() {
            return extract_join(handle);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    tracing::warn!(
        ?direction,
        budget_ms = budget.as_millis() as u64,
        "supervisor: forwarder join exceeded budget; detaching thread"
    );
    drop(handle); // detach the underlying OS thread
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "forwarder join exceeded budget",
    ))
}

fn log_forwarder_outcome(direction: Direction, result: Option<io::Result<ForwarderExit>>) {
    match result {
        None => {}
        Some(Ok(exit)) => {
            tracing::debug!(?direction, ?exit, "supervisor: forwarder exited cleanly")
        }
        Some(Err(e)) => {
            tracing::warn!(?direction, error = %e, "supervisor: forwarder error after child exit")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::forward::{spawn_forwarder, Direction};
    use std::io::{self, Cursor};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::thread;
    use std::time::Duration;

    /// In-memory mock of a `portable_pty::Child` for unit tests.
    /// State machine: `try_wait` returns `None` `pending_polls`
    /// times, then returns `Some(status)`. `kill` flips the
    /// status to a synthetic SIGTERM (signum 15) and finishes
    /// pending polls immediately.
    struct MockChild {
        state: Arc<Mutex<MockState>>,
    }

    struct MockState {
        polls_remaining: usize,
        scheduled: ExitStatus,
        kills: usize,
        try_waits: usize,
    }

    impl MockChild {
        fn new(scheduled: ExitStatus, polls_until_exit: usize) -> Self {
            Self {
                state: Arc::new(Mutex::new(MockState {
                    polls_remaining: polls_until_exit,
                    scheduled,
                    kills: 0,
                    try_waits: 0,
                })),
            }
        }

        fn handle(&self) -> Arc<Mutex<MockState>> {
            Arc::clone(&self.state)
        }
    }

    impl SupervisedChild for MockChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            let mut s = self.state.lock().unwrap();
            s.try_waits += 1;
            if s.polls_remaining == 0 {
                Ok(Some(s.scheduled.clone()))
            } else {
                s.polls_remaining -= 1;
                Ok(None)
            }
        }
        fn wait(&mut self) -> io::Result<ExitStatus> {
            let mut s = self.state.lock().unwrap();
            // Block-ish wait: just return the scheduled status.
            s.polls_remaining = 0;
            Ok(s.scheduled.clone())
        }
        fn clone_killer(&mut self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(MockKiller {
                state: Arc::clone(&self.state),
            })
        }
    }

    struct MockKiller {
        state: Arc<Mutex<MockState>>,
    }

    impl std::fmt::Debug for MockKiller {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockKiller").finish_non_exhaustive()
        }
    }

    impl ChildKiller for MockKiller {
        fn kill(&mut self) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            s.kills += 1;
            // Once killed, status becomes SIGTERM and try_wait
            // returns immediately on the next call.
            s.scheduled = ExitStatus::with_signal("Terminated");
            s.polls_remaining = 0;
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(MockKiller {
                state: Arc::clone(&self.state),
            })
        }
    }

    fn fast_deadlines() -> Deadlines {
        Deadlines {
            child_wait_deadline: Some(Duration::from_secs(2)),
            forwarder_join_budget: Duration::from_millis(500),
            wait_poll: Duration::from_millis(2),
        }
    }

    /// Build an `IoPump` whose forwarders both terminate
    /// instantly (empty source, sink discards everything).
    fn quiet_pump() -> IoPump {
        let h2c_in: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let h2c_out: Vec<u8> = Vec::new();
        let host_to_child = spawn_forwarder(Direction::HostToChild, h2c_in, h2c_out);

        let c2h_in: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let c2h_out: Vec<u8> = Vec::new();
        let child_to_host = spawn_forwarder(Direction::ChildToHost, c2h_in, c2h_out);

        IoPump {
            host_to_child,
            child_to_host,
        }
    }

    /// Build a pump whose forwarders never finish (block on a
    /// channel that's never closed). Used for "join exceeds
    /// budget" tests.
    fn hung_pump() -> IoPump {
        struct ForeverReader;
        impl io::Read for ForeverReader {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                std::thread::sleep(Duration::from_secs(60));
                Ok(0)
            }
        }
        struct DiscardWriter;
        impl io::Write for DiscardWriter {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let host_to_child = spawn_forwarder(Direction::HostToChild, ForeverReader, DiscardWriter);
        let child_to_host = spawn_forwarder(Direction::ChildToHost, ForeverReader, DiscardWriter);
        IoPump {
            host_to_child,
            child_to_host,
        }
    }

    #[test]
    fn clean_exit_returns_success_and_disarms_guard() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let handle = child.handle();
        let code = run_pump_session_with(child, quiet_pump(), fast_deadlines())
            .expect("clean exit must be Ok");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            handle.lock().unwrap().kills,
            0,
            "must not kill on clean exit"
        );
    }

    #[test]
    fn nonzero_exit_passes_through() {
        let child = MockChild::new(ExitStatus::with_exit_code(7), 0);
        let code = run_pump_session_with(child, quiet_pump(), fast_deadlines())
            .expect("nonzero exit must be Ok");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(7)));
    }

    #[test]
    fn signal_status_propagates_as_signal_error() {
        let child = MockChild::new(ExitStatus::with_signal("Terminated"), 0);
        let err = run_pump_session_with(child, quiet_pump(), fast_deadlines())
            .expect_err("signal must be Err");
        assert!(matches!(err, Error::Signal { signum: 15 }));
    }

    #[test]
    fn both_forwarders_converging_while_child_alive_escalates_kill() {
        // RD finding 1 regression guard: with quiet (instantly-
        // finishing) forwarders, the child takes many polls to
        // exit, so the supervisor must observe both forwarders
        // finished and escalate kill.
        let child = MockChild::new(ExitStatus::with_exit_code(99), 1_000_000);
        let handle = child.handle();
        let _err = run_pump_session_with(child, quiet_pump(), fast_deadlines())
            .expect_err("kill rewrites status to SIGTERM (signal 15)");
        let s = handle.lock().unwrap();
        assert!(
            s.kills >= 1,
            "supervisor must escalate kill (got {} kills)",
            s.kills
        );
    }

    #[test]
    fn forwarder_join_timeout_does_not_break_supervisor() {
        // Exit immediately, so we reach Phase 2 with hung
        // forwarders. The supervisor must still return the
        // ExitCode (logging warn rather than blocking forever).
        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let pump = hung_pump();
        let deadlines = Deadlines {
            child_wait_deadline: Some(Duration::from_secs(2)),
            forwarder_join_budget: Duration::from_millis(50),
            wait_poll: Duration::from_millis(2),
        };
        // Catch the "both forwarders finish" escalation by NOT
        // letting them finish — but quiet pump finishes
        // instantly, so we use hung. With hung pump and
        // immediate child exit, escalation never triggers
        // because forwarders never converge. We reach Phase 2
        // and time out the joins.
        let start = Instant::now();
        let code = run_pump_session_with(child, pump, deadlines)
            .expect("clean exit must be Ok even with hung forwarders");
        let elapsed = start.elapsed();
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        // Two forwarders × 50ms budget + a bit of slack.
        assert!(
            elapsed < Duration::from_millis(500),
            "supervisor stalled on hung forwarders: {elapsed:?}"
        );
    }

    /// Forwarder produces an `io::Error` (writer that always
    /// errors) before the child exits → supervisor must kill
    /// the child and surface the error as `Error::Pty`.
    #[test]
    fn forwarder_error_before_child_exit_kills_and_propagates() {
        struct Once {
            done: Arc<AtomicUsize>,
        }
        impl io::Read for Once {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                if self.done.fetch_add(1, Ordering::SeqCst) == 0 && !b.is_empty() {
                    b[0] = b'x';
                    return Ok(1);
                }
                // Subsequent reads block forever to avoid
                // races where the reader EOFs first.
                std::thread::sleep(Duration::from_secs(60));
                Ok(0)
            }
        }
        struct Failing;
        impl io::Write for Failing {
            fn write(&mut self, _b: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic failure"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let h2c = spawn_forwarder(
            Direction::HostToChild,
            Once {
                done: Arc::new(AtomicUsize::new(0)),
            },
            Failing,
        );
        // c2h: never finishes so it doesn't trigger the
        // "both converged" path on its own.
        let c2h = spawn_forwarder(
            Direction::ChildToHost,
            Once {
                done: Arc::new(AtomicUsize::new(usize::MAX / 2)),
            },
            Vec::<u8>::new(),
        );
        let pump = IoPump {
            host_to_child: h2c,
            child_to_host: c2h,
        };

        // Child takes many polls so the forwarder error
        // observably precedes child exit.
        let child = MockChild::new(ExitStatus::with_exit_code(0), 1_000_000);
        let handle = child.handle();
        let err = run_pump_session_with(child, pump, fast_deadlines())
            .expect_err("forwarder error must surface");
        assert!(matches!(err, Error::Pty(_)), "got {err:?}");
        assert!(handle.lock().unwrap().kills >= 1);
    }

    #[test]
    fn kill_on_drop_guard_kills_when_armed() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let killer = child.clone_killer_for_test();
        let handle = child.handle();
        {
            let _g = KillOnDropGuard::armed(killer);
            // armed; will fire on drop
        }
        assert_eq!(handle.lock().unwrap().kills, 1);
    }

    #[test]
    fn kill_on_drop_guard_skips_kill_when_disarmed() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let killer = child.clone_killer_for_test();
        let handle = child.handle();
        {
            let mut g = KillOnDropGuard::armed(killer);
            g.disarm();
        }
        assert_eq!(handle.lock().unwrap().kills, 0);
    }

    #[test]
    fn kill_on_drop_guard_kills_on_panic_unwind() {
        // RD finding 4: panic-path semantics pinned via
        // catch_unwind. A panic between arm and disarm must
        // still kill the child.
        let kills = Arc::new(AtomicUsize::new(0));
        let kills_for_panic = Arc::clone(&kills);
        let r = std::panic::catch_unwind(move || {
            #[derive(Debug)]
            struct CountingKiller {
                kills: Arc<AtomicUsize>,
            }
            impl ChildKiller for CountingKiller {
                fn kill(&mut self) -> io::Result<()> {
                    self.kills.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
                fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
                    Box::new(CountingKiller {
                        kills: Arc::clone(&self.kills),
                    })
                }
            }
            let killer: Box<dyn ChildKiller + Send + Sync> = Box::new(CountingKiller {
                kills: kills_for_panic,
            });
            let _g = KillOnDropGuard::armed(killer);
            panic!("synthetic panic between arm and disarm");
        });
        assert!(r.is_err(), "panic must propagate");
        assert_eq!(kills.load(Ordering::SeqCst), 1, "guard must fire on panic");
    }

    impl MockChild {
        fn clone_killer_for_test(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(MockKiller {
                state: Arc::clone(&self.state),
            })
        }
    }

    /// Sanity check: harvest is non-blocking when the forwarder
    /// has not finished yet.
    #[test]
    fn harvest_leaves_unfinished_handle_in_place() {
        let pump = hung_pump();
        let mut h2c = Some(pump.host_to_child);
        let mut result: Option<io::Result<ForwarderExit>> = None;
        // Brief sleep to let the forwarder enter its blocking
        // read; even after that, is_finished() is false.
        thread::sleep(Duration::from_millis(20));
        harvest(&mut h2c, &mut result);
        assert!(h2c.is_some(), "handle must remain when not finished");
        assert!(result.is_none());
        // Detach to clean up the test thread.
        drop(h2c);
        // Drop pump.child_to_host too.
        drop(pump.child_to_host);
    }
}

// Real-PTY integration tests for the supervisor. Mirrors the
// bounded-deadline pattern from `tests/pty_smoke.rs`,
// `src/pty/spawn.rs::real_spawn`, and `src/pty/pump.rs::real_pump`
// (constants duplicated locally per repo convention).
#[cfg(all(test, unix))]
mod real_session {
    use super::*;
    use crate::pty::pump::start_io_pump;
    use crate::pty::spawn::spawn_child;
    use portable_pty::PtySize;
    use std::ffi::{OsStr, OsString};
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

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

    fn supervisor_deadlines() -> Deadlines {
        Deadlines {
            child_wait_deadline: Some(WAIT_BUDGET),
            forwarder_join_budget: JOIN_BUDGET,
            wait_poll: WAIT_POLL,
        }
    }

    /// Shared `Write` sink so the integration test can inspect
    /// what the `child_to_host` forwarder produced after the
    /// supervisor returns.
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

    /// Drive the supervisor end-to-end against a shell command
    /// fragment. Returns the supervisor result and the captured
    /// child->host bytes.
    fn run_against_sh(script: &str, host_stdin: Vec<u8>) -> (Result<ExitCode>, Vec<u8>) {
        let mut session = spawn_child(
            OsStr::new("/bin/sh"),
            &[OsString::from("-c"), OsString::from(script)],
            pty_size_80x24(),
        )
        .expect("spawn /bin/sh");
        let sink = SharedSink::default();
        let pump = start_io_pump(&mut session, Cursor::new(host_stdin), sink.clone())
            .expect("start_io_pump");
        let result = run_pump_session(session, pump);
        (result, sink.snapshot())
    }

    #[test]
    fn supervisor_propagates_clean_exit() {
        let (res, _) = run_against_sh("exit 0", Vec::new());
        let code = res.expect("clean exit must be Ok");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
    }

    #[test]
    fn supervisor_propagates_nonzero_exit() {
        let (res, _) = run_against_sh("exit 7", Vec::new());
        let code = res.expect("nonzero clean exit must be Ok");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(7)));
    }

    #[test]
    fn supervisor_propagates_signal_death() {
        // RD finding 6: `exec sleep 30` so the SIGTERM stops
        // sleep itself (the shell exec'd into it), avoiding a
        // shell zombie or timing race where the shell ignores
        // SIGTERM and waits for sleep.
        let mut session = spawn_child(
            OsStr::new("/bin/sh"),
            &[OsString::from("-c"), OsString::from("exec sleep 30")],
            pty_size_80x24(),
        )
        .expect("spawn /bin/sh");

        let mut term = session.child.clone_killer();
        let sink = SharedSink::default();
        let pump = start_io_pump(&mut session, Cursor::new(Vec::<u8>::new()), sink)
            .expect("start_io_pump");

        // Signal the child BEFORE handing the session to the
        // supervisor so the very first poll sees a signal
        // status. portable-pty's ChildKiller for unix actually
        // sends SIGHUP (verified in portable-pty 0.9.0
        // src/lib.rs:328), not SIGTERM, so we expect signum=1.
        term.kill().expect("send SIGHUP to child via clone_killer");
        let res = run_pump_session(session, pump);
        let err = res.expect_err("signal death must be Err");
        match err {
            Error::Signal { signum } => assert_eq!(
                signum, 1,
                "portable-pty ChildKiller sends SIGHUP=1; got {signum} \
                 (locale-dependent strsignal mapping?)"
            ),
            other => panic!("expected Signal, got {other:?}"),
        }
    }

    #[test]
    fn supervisor_converges_on_host_stdin_eof() {
        // /bin/cat stays alive until its PTY input EOFs. With a
        // 6-byte cursor the host_to_child forwarder drains
        // immediately, the writer is dropped, cat sees EOT,
        // and exits cleanly. The supervisor must converge.
        let (res, _) = run_against_sh("exec cat", b"hello\n".to_vec());
        let code = res.expect("cat must exit cleanly on host stdin EOF");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
    }

    #[test]
    fn supervisor_drains_buffered_child_output() {
        // RD finding 10: a child that writes output and then
        // exits non-zero must not have its output truncated by
        // the supervisor's drain phase. The captured sink
        // should contain "hello" AND the exit code must be 3.
        let (res, captured) = run_against_sh("printf hello && exit 3", Vec::new());
        let code = res.expect("nonzero exit must be Ok");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(3)));
        let s = String::from_utf8_lossy(&captured);
        assert!(
            s.contains("hello"),
            "buffered child output truncated: captured = {s:?}"
        );
    }

    // Suppress warnings for unused supervisor_deadlines until a
    // future test needs the custom budget shape; production
    // entry uses Deadlines::production() under the hood.
    #[allow(dead_code)]
    fn _keep_deadlines_used() -> Deadlines {
        supervisor_deadlines()
    }
}
