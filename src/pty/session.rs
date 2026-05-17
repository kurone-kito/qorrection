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
//! ## Post-exit host-input cancellation
//!
//! `host_to_child` is the only forwarder that can still be
//! waiting on host input after the child exits. Issue #89 wires a
//! cancellation token into that direction so the supervisor can
//! ask the forwarder to break out of its host-stdin wait before
//! attempting the bounded post-exit join. The join budgets remain
//! split because the two directions still have different drain
//! roles:
//!
//! - [`Deadlines::forwarder_join_budget`] (5 s in production)
//!   is the budget for the `ChildToHost` direction. If the
//!   join exceeds that budget we emit `tracing::warn!`, drop
//!   the join handle (detaching the OS thread), and let the
//!   parent process termination clean it up.
//! - [`Deadlines::host_to_child_post_exit_budget`] is a short
//!   bounded join window for the cancelled `HostToChild`
//!   direction when cancellation is known to wake the blocked
//!   read (the unix pollable stdin path, or a test seam that
//!   models the same behavior). Once cancellation is signalled
//!   there is no useful user input left to deliver, so the budget
//!   only needs to be long enough for the poll loop to observe
//!   the cancel bit and exit cleanly instead of detaching
//!   immediately. Non-wakeable readers still detach with a zero
//!   base budget once no trigger animation is in flight, but an
//!   in-progress render now extends the join window long enough
//!   to restore the primary screen before teardown.

use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use portable_pty::{ChildKiller, ExitStatus};
#[cfg(unix)]
use portable_pty::{MasterPty, PtySize};

use crate::pty::exit::map_exit_status;
use crate::pty::forward::{Direction, ForwarderExit, ForwarderHandle};
use crate::pty::pump::IoPump;
#[cfg(unix)]
use crate::pty::size;
use crate::pty::spawn::SpawnedSession;
#[cfg(unix)]
use crate::signals::{Event as SignalEvent, SignalGuard};
use crate::trigger::input::SharedRenderProgress;
use crate::{Error, Result};

/// Polling and join time budgets the supervisor honors.
///
/// `child_wait_deadline = None` is the production setting — the
/// loop polls forever, with convergence guaranteed by either
/// (a) the child exits naturally or (b) both forwarders converge
/// and the supervisor escalates `kill()`. A finite deadline is
/// only meaningful in tests that need bounded run time.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadlines {
    /// Optional wall-clock deadline on Phase 1 (waiting for the
    /// child). `None` in production (poll forever). Tests pass
    /// a tight value (e.g. 50ms) for bounded runtime.
    pub child_wait_deadline: Option<Duration>,
    /// Maximum time spent joining the `ChildToHost` forwarder
    /// thread after the child has exited. On timeout the join
    /// handle is dropped and a `warn!` is emitted (detached-
    /// thread mode). Drains pending child output that the
    /// forwarder may still be writing to host stdout.
    pub forwarder_join_budget: Duration,
    /// Same as `forwarder_join_budget` but for the cancelled
    /// `HostToChild` direction when cancellation can wake the
    /// blocked read. Production keeps this short: once the child
    /// has exited the host->child forwarder has no useful work
    /// left, so the budget only needs to cover one or two poll
    /// intervals of the wakeable cancel loop. Non-wakeable
    /// readers still use a zero base budget unless the shared
    /// render-progress counter says the forwarder is finishing an
    /// in-flight trigger animation.
    pub host_to_child_post_exit_budget: Duration,
    /// Sleep between successive `try_wait` ticks. Keeps the
    /// supervisor from busy-looping; small enough that signal
    /// death is observed promptly.
    pub wait_poll: Duration,
    /// After the supervisor escalates a kill, how long it polls
    /// `try_wait()` before giving up. Bounds the failure path
    /// when a child catches/ignores the kill signal — without
    /// this, a blocking `child.wait()` could hang indefinitely
    /// and defeat the coupled-state-machine convergence
    /// guarantee.
    pub post_kill_wait_budget: Duration,
}

impl Deadlines {
    /// Production defaults: poll forever, 5s join budget, 20ms
    /// poll cadence. These match the rest of the PTY layer's
    /// per-file constants (see `pty/spawn.rs::real_spawn`,
    /// `pty/pump.rs::real_pump`).
    pub(crate) const fn production() -> Self {
        Self {
            child_wait_deadline: None,
            forwarder_join_budget: Duration::from_secs(5),
            // Give the cancelled host->stdin reader enough time
            // to observe the cancel bit and join cleanly without
            // reintroducing the old multi-second post-exit hang.
            host_to_child_post_exit_budget: Duration::from_millis(250),
            wait_poll: Duration::from_millis(20),
            post_kill_wait_budget: Duration::from_secs(5),
        }
    }
}

#[cfg(unix)]
pub(crate) enum SessionStatus<C = PtyChild> {
    Exited(ExitCode),
    DeferredShutdown(Box<DeferredShutdown<C>>),
}

#[cfg(not(unix))]
pub(crate) enum SessionStatus {
    Exited(ExitCode),
}

enum SupervisorOutcome<C> {
    Exited(ExitCode),
    Shutdown(PendingShutdown<C>),
}

struct PendingShutdown<C> {
    child: C,
    host_to_child: Option<ForwarderHandle>,
    child_to_host: Option<ForwarderHandle>,
    host_to_child_render_progress: Option<SharedRenderProgress>,
    h2c_result: Option<io::Result<ForwarderExit>>,
    c2h_result: Option<io::Result<ForwarderExit>>,
    deadlines: Deadlines,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
enum ShutdownTarget {
    ProcessGroup(libc::pid_t),
    Process(libc::pid_t),
}

#[cfg(unix)]
impl ShutdownTarget {
    fn from_parts(
        process_group_leader: Option<libc::pid_t>,
        process_id: Option<u32>,
    ) -> Option<Self> {
        if let Some(process_group_leader) = process_group_leader.filter(|pid| *pid > 0) {
            return Some(Self::ProcessGroup(process_group_leader));
        }

        process_id
            .and_then(|pid| libc::pid_t::try_from(pid).ok())
            .filter(|pid| *pid > 0)
            .map(Self::Process)
    }
}

#[cfg(unix)]
pub(crate) struct DeferredShutdown<C> {
    child: C,
    master: Box<dyn MasterPty + Send>,
    signal_guard: Option<SignalGuard>,
    host_to_child: Option<ForwarderHandle>,
    child_to_host: Option<ForwarderHandle>,
    host_to_child_render_progress: Option<SharedRenderProgress>,
    h2c_result: Option<io::Result<ForwarderExit>>,
    c2h_result: Option<io::Result<ForwarderExit>>,
    deadlines: Deadlines,
}

#[cfg(unix)]
impl<C> DeferredShutdown<C>
where
    C: SupervisedChild,
{
    fn resolve_shutdown_target(&self) -> Option<ShutdownTarget> {
        ShutdownTarget::from_parts(self.master.process_group_leader(), self.child.process_id())
    }

    pub(crate) fn complete(mut self) -> Result<ExitCode> {
        let mut guard = KillOnDropGuard::armed(self.child.clone_killer());
        if let Some(status) = self
            .child
            .try_wait()
            .map_err(|e| wrap_io("child try_wait before forwarded SIGTERM", e))?
        {
            guard.disarm();
            return finish_after_child_exit(
                status,
                self.host_to_child.take(),
                self.child_to_host.take(),
                self.host_to_child_render_progress.take(),
                self.h2c_result.take(),
                self.c2h_result.take(),
                self.deadlines,
            );
        }
        let target = self.resolve_shutdown_target().ok_or_else(|| {
            wrap_io(
                "forward SIGTERM to PTY child",
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "no PTY process group leader or child pid was available",
                ),
            )
        })?;
        signal_shutdown_target(target).map_err(|e| wrap_io("forward SIGTERM to PTY child", e))?;
        let status = wait_with_budget(
            &mut self.child,
            self.deadlines.post_kill_wait_budget,
            self.deadlines.wait_poll,
        )?
        .ok_or_else(|| {
            wrap_io(
                "wait for PTY child after forwarded SIGTERM",
                io::Error::new(io::ErrorKind::TimedOut, "post-SIGTERM wait budget exceeded"),
            )
        })?;
        guard.disarm();

        finish_after_child_exit(
            status,
            self.host_to_child.take(),
            self.child_to_host.take(),
            self.host_to_child_render_progress.take(),
            self.h2c_result.take(),
            self.c2h_result.take(),
            self.deadlines,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickAction {
    Continue,
    Shutdown,
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
    /// Produce a fresh killer handle. Wraps
    /// `portable_pty::Child::clone_killer`.
    fn clone_killer(&mut self) -> Box<dyn ChildKiller + Send + Sync>;
    /// Report the child pid when the backend exposes one.
    fn process_id(&self) -> Option<u32>;
}

/// Production [`SupervisedChild`] over a `portable-pty` child.
pub(crate) struct PtyChild {
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl SupervisedChild for PtyChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
    fn clone_killer(&mut self) -> Box<dyn ChildKiller + Send + Sync> {
        self.child.clone_killer()
    }
    fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }
}

#[cfg(unix)]
trait ResizeTarget {
    fn resize_pty(&self, size: PtySize) -> anyhow::Result<()>;
}

#[cfg(unix)]
impl<T> ResizeTarget for T
where
    T: MasterPty + ?Sized,
{
    fn resize_pty(&self, size: PtySize) -> anyhow::Result<()> {
        self.resize(size)
    }
}

#[cfg(unix)]
trait SignalSource {
    fn drain_events(&self) -> io::Result<Vec<SignalEvent>>;
}

#[cfg(unix)]
impl SignalSource for SignalGuard {
    fn drain_events(&self) -> io::Result<Vec<SignalEvent>> {
        self.drain()
    }
}

/// RAII guard that best-effort `kill()`s the child unless
/// disarmed. Documented invariant: armed until a successful
/// wait returns and consumes a status. See module-level docs.
pub(crate) struct KillOnDropGuard {
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
}

impl KillOnDropGuard {
    pub(crate) fn armed(killer: Box<dyn ChildKiller + Send + Sync>) -> Self {
        Self {
            killer: Some(killer),
        }
    }

    pub(crate) fn disarm(&mut self) {
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
pub(crate) fn run_pump_session(session: SpawnedSession, pump: IoPump) -> Result<SessionStatus> {
    let SpawnedSession { child, master } = session;
    #[cfg(unix)]
    {
        run_pump_session_unix(child, master, pump)
    }
    #[cfg(not(unix))]
    {
        let child = PtyChild { child };
        let _master = master;
        let code = run_pump_session_with(child, pump, Deadlines::production())?;
        Ok(SessionStatus::Exited(code))
    }
}

#[cfg(unix)]
fn run_pump_session_unix(
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    pump: IoPump,
) -> Result<SessionStatus<PtyChild>> {
    run_pump_session_unix_with_setup(child, master, pump, SignalGuard::install)
}

#[cfg(unix)]
fn run_pump_session_unix_with_setup<Setup>(
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    pump: IoPump,
    setup: Setup,
) -> Result<SessionStatus<PtyChild>>
where
    Setup: FnOnce() -> Result<SignalGuard>,
{
    let mut kill_guard = KillOnDropGuard::armed(child.clone_killer());
    let child = PtyChild { child };
    // Bind the master to a named local so it stays alive for
    // the entire supervised session. A wildcard (`master: _`)
    // pattern would drop it immediately at the destructuring
    // point, EOF'ing the slave side and racing the child / the
    // forwarder threads. The named binding extends its lifetime
    // to the end of the function, so the master is dropped only
    // after the supervisor returns.
    let signal_guard = setup()?;
    kill_guard.disarm();
    match run_pump_session_with_signals(
        child,
        master,
        &signal_guard,
        size::current_size,
        pump,
        Deadlines::production(),
    )? {
        SessionStatus::Exited(code) => Ok(SessionStatus::Exited(code)),
        SessionStatus::DeferredShutdown(mut shutdown) => {
            shutdown.signal_guard = Some(signal_guard);
            Ok(SessionStatus::DeferredShutdown(shutdown))
        }
    }
}

/// Lifecycle core, parameterised over the [`SupervisedChild`]
/// implementation and the time [`Deadlines`]. Exists so unit
/// tests can drive every branch of the state machine without a
/// real PTY.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn run_pump_session_with<C>(
    child: C,
    pump: IoPump,
    deadlines: Deadlines,
) -> Result<ExitCode>
where
    C: SupervisedChild,
{
    match run_pump_session_inner(child, pump, deadlines, || Ok(TickAction::Continue))? {
        SupervisorOutcome::Exited(code) => Ok(code),
        SupervisorOutcome::Shutdown(_) => unreachable!("shutdown requires a signal source"),
    }
}

#[cfg(unix)]
fn run_pump_session_with_signals<C, S, Q>(
    child: C,
    master: Box<dyn MasterPty + Send>,
    signals: &S,
    mut query_size: Q,
    pump: IoPump,
    deadlines: Deadlines,
) -> Result<SessionStatus<C>>
where
    C: SupervisedChild,
    S: SignalSource + ?Sized,
    Q: FnMut() -> Option<PtySize>,
{
    match run_pump_session_inner(child, pump, deadlines, || {
        handle_signal_events(signals, master.as_ref(), &mut query_size)
    })? {
        SupervisorOutcome::Exited(code) => Ok(SessionStatus::Exited(code)),
        SupervisorOutcome::Shutdown(pending) => Ok(SessionStatus::DeferredShutdown(Box::new(
            DeferredShutdown {
                child: pending.child,
                master,
                signal_guard: None,
                host_to_child: pending.host_to_child,
                child_to_host: pending.child_to_host,
                host_to_child_render_progress: pending.host_to_child_render_progress,
                h2c_result: pending.h2c_result,
                c2h_result: pending.c2h_result,
                deadlines: pending.deadlines,
            },
        ))),
    }
}

fn run_pump_session_inner<C, Tick>(
    mut child: C,
    pump: IoPump,
    deadlines: Deadlines,
    mut on_tick: Tick,
) -> Result<SupervisorOutcome<C>>
where
    C: SupervisedChild,
    Tick: FnMut() -> Result<TickAction>,
{
    let mut guard = KillOnDropGuard::armed(child.clone_killer());

    let (host_to_child, child_to_host, host_to_child_render_progress) = pump.into_parts();
    let mut h2c = Some(host_to_child);
    let mut c2h = Some(child_to_host);
    let mut h2c_result: Option<io::Result<ForwarderExit>> = None;
    let mut c2h_result: Option<io::Result<ForwarderExit>> = None;

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return Err(wrap_io("child try_wait", e)),
        }
        if matches!(on_tick()?, TickAction::Shutdown) {
            guard.disarm();
            return Ok(SupervisorOutcome::Shutdown(PendingShutdown {
                child,
                host_to_child: h2c,
                child_to_host: c2h,
                host_to_child_render_progress,
                h2c_result,
                c2h_result,
                deadlines,
            }));
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
            // exits is unrecoverable. Escalate, wait, surface.
            // Surface kill failure rather than blocking on
            // wait() when the child may be unreachable.
            if let Err(e) = escalate_kill(&mut child) {
                // Leave the guard armed: the child is likely
                // still running and drop() is the last defense.
                return Err(wrap_io("forwarder failed; kill escalation failed", e));
            }
            // Bounded wait: a child catching/ignoring the kill
            // signal must not be allowed to hang the supervisor.
            // Only disarm when an exit status was actually
            // observed; otherwise leave drop() to retry the kill.
            match wait_with_budget(
                &mut child,
                deadlines.post_kill_wait_budget,
                deadlines.wait_poll,
            ) {
                Ok(Some(_status)) => guard.disarm(),
                Ok(None) => {
                    tracing::warn!(
                        budget = ?deadlines.post_kill_wait_budget,
                        "supervisor: child did not exit within post-kill wait budget after forwarder error"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "supervisor: wait_with_budget() after kill failed");
                }
            }
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
            // alive → no producer, no consumer. Escalate and
            // block on `wait()`. If the kill itself failed, the
            // child may be unreachable; surface the error rather
            // than blocking forever.
            if let Err(e) = escalate_kill(&mut child) {
                // Leave the guard armed: the child is likely
                // still running and drop() is the last defense.
                return Err(wrap_io("stalled supervisor; kill escalation failed", e));
            }
            match wait_with_budget(
                &mut child,
                deadlines.post_kill_wait_budget,
                deadlines.wait_poll,
            )? {
                Some(s) => break s,
                None => {
                    return Err(wrap_io(
                        "stalled supervisor; child did not exit within post-kill budget",
                        io::Error::new(io::ErrorKind::TimedOut, "post-kill wait budget exceeded"),
                    ));
                }
            }
        }

        if let Some(d) = deadlines.child_wait_deadline {
            if start.elapsed() >= d {
                // Test-only convergence path: bail by killing
                // and waiting. Production passes None.
                if let Err(e) = escalate_kill(&mut child) {
                    // Leave the guard armed: the child is likely
                    // still running and drop() is the last defense.
                    return Err(wrap_io("deadline reached; kill escalation failed", e));
                }
                match wait_with_budget(
                    &mut child,
                    deadlines.post_kill_wait_budget,
                    deadlines.wait_poll,
                )? {
                    Some(s) => break s,
                    None => {
                        return Err(wrap_io(
                            "deadline reached; child did not exit within post-kill budget",
                            io::Error::new(
                                io::ErrorKind::TimedOut,
                                "post-kill wait budget exceeded",
                            ),
                        ));
                    }
                }
            }
        }

        std::thread::sleep(deadlines.wait_poll);
    };

    // Wait consumed a status → guard's job is done.
    guard.disarm();

    let code = finish_after_child_exit(
        status,
        h2c.take(),
        c2h.take(),
        host_to_child_render_progress,
        h2c_result,
        c2h_result,
        deadlines,
    )?;
    Ok(SupervisorOutcome::Exited(code))
}

#[cfg(unix)]
fn handle_signal_events<S, R, Q>(signals: &S, resizer: &R, query_size: &mut Q) -> Result<TickAction>
where
    S: SignalSource + ?Sized,
    R: ResizeTarget + ?Sized,
    Q: FnMut() -> Option<PtySize>,
{
    let mut saw_resize = false;
    for event in signals
        .drain_events()
        .map_err(|e| wrap_io("signal drain", e))?
    {
        match event {
            SignalEvent::Resize => saw_resize = true,
            SignalEvent::Shutdown => return Ok(TickAction::Shutdown),
        }
    }

    if !saw_resize {
        return Ok(TickAction::Continue);
    }

    let Some(size) = query_size() else {
        tracing::debug!("supervisor: skipping SIGWINCH resize because host size was unavailable");
        return Ok(TickAction::Continue);
    };

    resizer
        .resize_pty(size)
        .map_err(|e| Error::Pty(e.context("apply SIGWINCH PTY resize")))?;
    Ok(TickAction::Continue)
}

fn wrap_io(context: &'static str, e: io::Error) -> Error {
    Error::Pty(anyhow::Error::new(e).context(context))
}

fn escalate_kill<C: SupervisedChild>(child: &mut C) -> io::Result<()> {
    let mut k = child.clone_killer();
    if let Err(e) = k.kill() {
        tracing::warn!(error = %e, "supervisor: best-effort kill failed");
        return Err(e);
    }
    Ok(())
}

/// Bounded post-kill wait: poll `try_wait()` for up to `budget`,
/// sleeping `poll` between ticks. Returns `Ok(Some(status))` if
/// the child exits in time, `Ok(None)` if the budget elapses
/// (caller surfaces a timeout), or `Err` on a `try_wait` failure.
fn wait_with_budget<C: SupervisedChild>(
    child: &mut C,
    budget: Duration,
    poll: Duration,
) -> Result<Option<ExitStatus>> {
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Ok(Some(s)),
            Ok(None) => {}
            Err(e) => return Err(wrap_io("child try_wait after kill", e)),
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(poll);
    }
}

#[cfg(unix)]
fn signal_shutdown_target(target: ShutdownTarget) -> io::Result<()> {
    let rc = match target {
        ShutdownTarget::ProcessGroup(process_group_leader) => {
            // SAFETY: `ShutdownTarget::from_parts()` constructs this variant only
            // from positive `pid_t` values, so the group leader id is well-formed
            // for libc. Sending SIGTERM is intentional, and errno handling is
            // preserved below.
            unsafe { libc::killpg(process_group_leader, libc::SIGTERM) }
        }
        ShutdownTarget::Process(process_id) => {
            // SAFETY: `ShutdownTarget::from_parts()` constructs this variant only
            // from positive `pid_t` values converted from the child pid, so the
            // process id is well-formed for libc. Sending SIGTERM is intentional,
            // and errno handling is preserved below.
            unsafe { libc::kill(process_id, libc::SIGTERM) }
        }
    };
    if rc == 0 {
        return Ok(());
    }

    let err = io::Error::last_os_error();
    if matches!(err.raw_os_error(), Some(code) if code == libc::ESRCH) {
        tracing::debug!(
            ?target,
            error = %err,
            "supervisor: shutdown target exited before SIGTERM forward"
        );
        return Ok(());
    }

    Err(err)
}

fn finish_after_child_exit(
    status: ExitStatus,
    mut host_to_child: Option<ForwarderHandle>,
    mut child_to_host: Option<ForwarderHandle>,
    host_to_child_render_progress: Option<SharedRenderProgress>,
    mut h2c_result: Option<io::Result<ForwarderExit>>,
    mut c2h_result: Option<io::Result<ForwarderExit>>,
    deadlines: Deadlines,
) -> Result<ExitCode> {
    // Phase 2: drain remaining forwarders within budget.
    if let Some(h) = host_to_child.take() {
        h.cancel();
        let base_budget = if h.cancel_wakes_read() {
            deadlines.host_to_child_post_exit_budget
        } else {
            Duration::ZERO
        };
        let budget = IoPump::host_to_child_post_exit_budget(
            host_to_child_render_progress.as_ref(),
            base_budget,
        );
        h2c_result = Some(join_with_budget(h, budget));
    }
    if let Some(h) = child_to_host.take() {
        c2h_result = Some(join_with_budget(h, deadlines.forwarder_join_budget));
    }
    log_forwarder_outcome(Direction::HostToChild, h2c_result);
    log_forwarder_outcome(Direction::ChildToHost, c2h_result);

    map_exit_status(status)
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
    let direction = handle.direction;
    // Always try a non-blocking check first so a zero-budget call
    // still extracts a finished forwarder's outcome instead of
    // being treated as a timeout.
    if handle.join.is_finished() {
        return extract_join(handle);
    }
    if budget.is_zero() {
        // Intentional zero-budget detach. Keep this branch for
        // test seams that want an immediate detach path without
        // waiting for the poll loop.
        tracing::debug!(
            ?direction,
            "supervisor: zero-budget detach of unfinished forwarder"
        );
        drop(handle);
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "forwarder detached with zero budget",
        ));
    }
    let deadline = Instant::now() + budget;
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
        Some(Err(e)) if e.kind() == io::ErrorKind::TimedOut => {
            // join_with_budget has already logged the detach
            // (warn for budget exceeded, debug for zero-budget).
            // Avoid double-logging the same condition here.
        }
        Some(Err(e)) => {
            tracing::warn!(?direction, error = %e, "supervisor: forwarder error after child exit")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::forward::{
        spawn_cancellable_forwarder, spawn_forwarder, CancelHandle, CancellableReader, Direction,
    };
    #[cfg(unix)]
    use crate::signals::TEST_SERIAL;
    use std::io::{self, Cursor};
    #[cfg(unix)]
    use std::sync::atomic::AtomicI32;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::thread;
    use std::time::{Duration, Instant};
    #[cfg(unix)]
    use std::{cell::RefCell, collections::VecDeque};

    /// In-memory mock of a `portable_pty::Child` for unit tests.
    /// State machine: `try_wait` returns `None` `pending_polls`
    /// times, then returns `Some(status)`. `kill` flips the
    /// status to a synthetic SIGTERM (signum 15) and finishes
    /// pending polls immediately.
    struct MockChild {
        state: Arc<Mutex<MockState>>,
    }

    #[derive(Debug)]
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
        fn clone_killer(&mut self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(MockKiller {
                state: Arc::clone(&self.state),
            })
        }
        fn process_id(&self) -> Option<u32> {
            Some(42_042)
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
            s.scheduled = ExitStatus::with_signal("Signal 15");
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
            host_to_child_post_exit_budget: Duration::from_millis(500),
            wait_poll: Duration::from_millis(2),
            post_kill_wait_budget: Duration::from_millis(500),
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
            host_to_child_render_progress: None,
        }
    }

    /// Build a pump whose forwarders never finish (block forever
    /// in `read()`). Used for "join exceeds budget" tests; the
    /// surrounding test must rely on the supervisor's bounded
    /// budgets (kill + join detach) for termination.
    fn hung_pump() -> IoPump {
        struct ForeverReader;
        impl io::Read for ForeverReader {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                loop {
                    std::thread::park_timeout(Duration::from_secs(60));
                }
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
            host_to_child_render_progress: None,
        }
    }

    /// Build a pump where `HostToChild` waits until the
    /// supervisor signals cancellation, while `ChildToHost`
    /// terminates instantly. The returned flag flips when the
    /// blocked reader observed cancellation and exited.
    fn cancellable_h2c_quiet_c2h_pump() -> (IoPump, Arc<AtomicBool>) {
        struct CancelAwareForeverReader {
            cancel: CancelHandle,
            started: Arc<AtomicBool>,
            exited: Arc<AtomicBool>,
        }
        impl io::Read for CancelAwareForeverReader {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                self.started.store(true, Ordering::SeqCst);
                while !self.cancel.is_cancelled() {
                    std::thread::park_timeout(Duration::from_millis(10));
                }
                self.exited.store(true, Ordering::SeqCst);
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
        let host_cancel = CancelHandle::new();
        let started = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let host_to_child = spawn_cancellable_forwarder(
            Direction::HostToChild,
            CancellableReader::new(
                CancelAwareForeverReader {
                    cancel: host_cancel.clone(),
                    started: Arc::clone(&started),
                    exited: Arc::clone(&exited),
                },
                host_cancel,
            ),
            DiscardWriter,
            true,
        );
        let c2h_in: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let c2h_out: Vec<u8> = Vec::new();
        let child_to_host = spawn_forwarder(Direction::ChildToHost, c2h_in, c2h_out);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            started.load(Ordering::SeqCst),
            "host->child forwarder did not enter read() within the wait budget"
        );

        (
            IoPump {
                host_to_child,
                child_to_host,
                host_to_child_render_progress: None,
            },
            exited,
        )
    }

    /// Build a pump where `HostToChild` ignores cancellation once
    /// its blocking read begins. This models the non-wakeable
    /// fallback path used by non-Unix stdin.
    fn nonwakeable_h2c_quiet_c2h_pump() -> IoPump {
        struct ForeverReader;
        impl io::Read for ForeverReader {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                loop {
                    std::thread::park_timeout(Duration::from_secs(60));
                }
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
        let host_cancel = CancelHandle::new();
        let host_to_child = spawn_cancellable_forwarder(
            Direction::HostToChild,
            CancellableReader::new(ForeverReader, host_cancel),
            DiscardWriter,
            false,
        );
        let c2h_in: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let c2h_out: Vec<u8> = Vec::new();
        let child_to_host = spawn_forwarder(Direction::ChildToHost, c2h_in, c2h_out);
        IoPump {
            host_to_child,
            child_to_host,
            host_to_child_render_progress: None,
        }
    }

    /// Regression for chatgpt-codex/copilot reviewer finding
    /// (PR #91): zero budget must NOT bypass the
    /// `is_finished()` extraction — a HostToChild forwarder
    /// that has already finished by the time the supervisor
    /// reaches the join site must surface its real outcome
    /// instead of being treated as a timed-out detach.
    #[test]
    fn zero_budget_extracts_finished_forwarder() {
        // EOF input + no-op writer => forwarder finishes
        // essentially immediately.
        let reader: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let writer: Vec<u8> = Vec::new();
        let handle = spawn_forwarder(Direction::HostToChild, reader, writer);
        // Bounded spin instead of a fixed sleep: a contended CI
        // runner can take longer than any constant we'd pick
        // here, so poll `is_finished()` up to a generous
        // deadline before exercising the zero-budget branch.
        let wait_deadline = Instant::now() + Duration::from_secs(2);
        while !handle.join.is_finished() && Instant::now() < wait_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.join.is_finished(),
            "forwarder did not reach completion within the wait budget"
        );
        let result = join_with_budget(handle, Duration::ZERO);
        assert!(
            result.is_ok(),
            "zero-budget join of finished forwarder must extract Ok, got {result:?}"
        );
    }

    /// Regression for issue #89: with production deadlines, an
    /// interactive `q9` session whose `HostToChild` reader is
    /// blocked on host input must be cancelled and joined within
    /// the short post-exit budget rather than hanging until the
    /// child->host budget expires.
    #[test]
    fn production_deadlines_cancel_hung_host_to_child_before_join() {
        // child_wait_deadline=None in production; switch to a
        // bounded value here so the test cannot hang on a
        // try_wait regression. Keep the per-direction budgets
        // exactly as production() sets them.
        let mut deadlines = Deadlines::production();
        deadlines.child_wait_deadline = Some(Duration::from_secs(2));
        deadlines.wait_poll = Duration::from_millis(2);

        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let (pump, exited) = cancellable_h2c_quiet_c2h_pump();
        let start = Instant::now();
        let code = run_pump_session_with(child, pump, deadlines)
            .expect("clean exit with hung h2c must still be Ok");
        let elapsed = start.elapsed();

        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert!(
            exited.load(Ordering::SeqCst),
            "host->child reader should observe cancellation before supervisor returns"
        );
        // Generous slack but well under the old multi-second
        // budget the degraded mode would have hit.
        assert!(
            elapsed < Duration::from_millis(500),
            "supervisor stalled on hung HostToChild: {elapsed:?}"
        );
    }

    /// Regression for the accepted E-phase review on PR #125:
    /// non-wakeable host stdin must retain the old zero-budget
    /// detach path when no render is in flight so non-Unix
    /// production builds do not wait the short pollable-join
    /// budget and emit a spurious timeout warning.
    #[test]
    fn production_deadlines_keep_zero_budget_for_nonwakeable_host_to_child() {
        let mut deadlines = Deadlines::production();
        deadlines.child_wait_deadline = Some(Duration::from_secs(2));
        deadlines.wait_poll = Duration::from_millis(2);

        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let start = Instant::now();
        let code = run_pump_session_with(child, nonwakeable_h2c_quiet_c2h_pump(), deadlines)
            .expect("clean exit with non-wakeable h2c must still be Ok");
        let elapsed = start.elapsed();

        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert!(
            elapsed < Duration::from_millis(100),
            "non-wakeable HostToChild should detach immediately: {elapsed:?}"
        );
    }

    /// Regression for PR #134 / issue #57: a non-wakeable
    /// HostToChild reader may already be inside `read()` while
    /// also presenting a trigger animation. Even though
    /// cancellation cannot wake that read, the supervisor still
    /// needs to honor the render-progress budget long enough for
    /// the in-flight animation to restore the primary screen
    /// before returning.
    #[test]
    fn production_deadlines_wait_for_inflight_nonwakeable_render() {
        struct DelayedReader {
            delay: Duration,
            started: Arc<AtomicBool>,
            exited: Arc<AtomicBool>,
        }
        impl io::Read for DelayedReader {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                self.started.store(true, Ordering::SeqCst);
                std::thread::sleep(self.delay);
                self.exited.store(true, Ordering::SeqCst);
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

        let mut deadlines = Deadlines::production();
        deadlines.child_wait_deadline = Some(Duration::from_secs(2));
        deadlines.wait_poll = Duration::from_millis(2);

        let host_cancel = CancelHandle::new();
        let started = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let host_to_child = spawn_cancellable_forwarder(
            Direction::HostToChild,
            CancellableReader::new(
                DelayedReader {
                    delay: Duration::from_millis(100),
                    started: Arc::clone(&started),
                    exited: Arc::clone(&exited),
                },
                host_cancel,
            ),
            DiscardWriter,
            false,
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            started.load(Ordering::SeqCst),
            "host->child forwarder did not enter read() within the wait budget"
        );

        let c2h_in: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let c2h_out: Vec<u8> = Vec::new();
        let child_to_host = spawn_forwarder(Direction::ChildToHost, c2h_in, c2h_out);
        let pump = IoPump {
            host_to_child,
            child_to_host,
            host_to_child_render_progress: Some(Arc::new(AtomicUsize::new(1))),
        };

        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let start = Instant::now();
        let code = run_pump_session_with(child, pump, deadlines)
            .expect("clean exit with in-flight non-wakeable render must still be Ok");
        let elapsed = start.elapsed();

        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert!(
            exited.load(Ordering::SeqCst),
            "supervisor should wait for the in-flight non-wakeable render to finish"
        );
        assert!(
            elapsed >= Duration::from_millis(80),
            "supervisor detached before the in-flight render finished: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "supervisor should wait only for the in-flight render, not the full budget: {elapsed:?}"
        );
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
        let child = MockChild::new(ExitStatus::with_signal("Signal 15"), 0);
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
            host_to_child_post_exit_budget: Duration::from_millis(50),
            wait_poll: Duration::from_millis(2),
            post_kill_wait_budget: Duration::from_millis(500),
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
                std::thread::sleep(Duration::from_millis(200));
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
            host_to_child_render_progress: None,
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

    #[cfg(unix)]
    #[derive(Debug)]
    struct PortableMockChild {
        state: Arc<Mutex<MockState>>,
    }

    #[cfg(unix)]
    impl portable_pty::ChildKiller for PortableMockChild {
        fn kill(&mut self) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            s.kills += 1;
            s.scheduled = ExitStatus::with_signal("Signal 15");
            s.polls_remaining = 0;
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(MockKiller {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[cfg(unix)]
    impl portable_pty::Child for PortableMockChild {
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
            Ok(self.state.lock().unwrap().scheduled.clone())
        }

        fn process_id(&self) -> Option<u32> {
            None
        }
    }

    #[cfg(unix)]
    #[derive(Debug)]
    struct DummyMaster;

    #[cfg(unix)]
    impl MasterPty for DummyMaster {
        fn resize(&self, _size: PtySize) -> anyhow::Result<()> {
            Ok(())
        }

        fn get_size(&self) -> anyhow::Result<PtySize> {
            Ok(pty_size(80, 24))
        }

        fn try_clone_reader(&self) -> anyhow::Result<Box<dyn io::Read + Send>> {
            Ok(Box::new(Cursor::new(Vec::<u8>::new())))
        }

        fn take_writer(&self) -> anyhow::Result<Box<dyn io::Write + Send>> {
            Ok(Box::new(Vec::<u8>::new()))
        }

        fn process_group_leader(&self) -> Option<libc::pid_t> {
            None
        }

        fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> {
            None
        }

        fn tty_name(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    #[cfg(unix)]
    #[derive(Default)]
    struct MockSignalSource {
        batches: RefCell<VecDeque<Vec<SignalEvent>>>,
    }

    #[cfg(unix)]
    impl MockSignalSource {
        fn new(batches: impl IntoIterator<Item = Vec<SignalEvent>>) -> Self {
            Self {
                batches: RefCell::new(batches.into_iter().collect()),
            }
        }
    }

    #[cfg(unix)]
    impl SignalSource for MockSignalSource {
        fn drain_events(&self) -> io::Result<Vec<SignalEvent>> {
            Ok(self.batches.borrow_mut().pop_front().unwrap_or_default())
        }
    }

    #[cfg(unix)]
    #[derive(Default)]
    struct MockMasterState {
        calls: Mutex<Vec<PtySize>>,
        fail: bool,
        process_group_leader: AtomicI32,
    }

    #[cfg(unix)]
    struct MockMaster {
        state: Arc<MockMasterState>,
    }

    #[cfg(unix)]
    impl MockMaster {
        fn shared(
            fail: bool,
            process_group_leader: Option<libc::pid_t>,
        ) -> (Self, Arc<MockMasterState>) {
            let state = Arc::new(MockMasterState {
                calls: Mutex::new(Vec::new()),
                fail,
                process_group_leader: AtomicI32::new(process_group_leader.unwrap_or_default()),
            });
            (
                Self {
                    state: Arc::clone(&state),
                },
                state,
            )
        }

        fn set_process_group_leader(
            state: &MockMasterState,
            process_group_leader: Option<libc::pid_t>,
        ) {
            state
                .process_group_leader
                .store(process_group_leader.unwrap_or_default(), Ordering::SeqCst);
        }
    }

    #[cfg(unix)]
    impl MasterPty for MockMaster {
        fn resize(&self, size: PtySize) -> anyhow::Result<()> {
            if self.state.fail {
                anyhow::bail!("synthetic resize failure");
            }
            self.state.calls.lock().unwrap().push(size);
            Ok(())
        }

        fn get_size(&self) -> anyhow::Result<PtySize> {
            Ok(pty_size(80, 24))
        }

        fn try_clone_reader(&self) -> anyhow::Result<Box<dyn io::Read + Send>> {
            Ok(Box::new(Cursor::new(Vec::<u8>::new())))
        }

        fn take_writer(&self) -> anyhow::Result<Box<dyn io::Write + Send>> {
            Ok(Box::new(Vec::<u8>::new()))
        }

        fn process_group_leader(&self) -> Option<libc::pid_t> {
            let pid = self.state.process_group_leader.load(Ordering::SeqCst);
            (pid > 0).then_some(pid)
        }

        fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> {
            None
        }

        fn tty_name(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    #[cfg(unix)]
    fn pty_size(cols: u16, rows: u16) -> PtySize {
        PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    #[cfg(unix)]
    fn expect_exited<C>(status: SessionStatus<C>) -> ExitCode {
        match status {
            SessionStatus::Exited(code) => code,
            SessionStatus::DeferredShutdown(_) => {
                panic!("signal-free resize path should not defer shutdown")
            }
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

    #[cfg(unix)]
    #[test]
    fn resize_events_collapse_to_latest_host_size() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 1);
        let signals = MockSignalSource::new([vec![
            SignalEvent::Resize,
            SignalEvent::Resize,
            SignalEvent::Resize,
        ]]);
        let (master, state) = MockMaster::shared(false, None);
        let mut query_calls = 0usize;
        let (pump, _exited) = cancellable_h2c_quiet_c2h_pump();

        let code = expect_exited(
            run_pump_session_with_signals(
                child,
                Box::new(master),
                &signals,
                || {
                    query_calls += 1;
                    Some(pty_size(132, 44))
                },
                pump,
                fast_deadlines(),
            )
            .expect("clean exit with resize must be Ok"),
        );

        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(query_calls, 1, "resize batch should query host size once");
        assert_eq!(state.calls.lock().unwrap().clone(), vec![pty_size(132, 44)]);
    }

    #[cfg(unix)]
    #[test]
    fn resize_events_ignore_unusable_runtime_size() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 1);
        let signals = MockSignalSource::new([vec![SignalEvent::Resize]]);
        let (master, state) = MockMaster::shared(false, None);
        let (pump, _exited) = cancellable_h2c_quiet_c2h_pump();

        let code = expect_exited(
            run_pump_session_with_signals(
                child,
                Box::new(master),
                &signals,
                || None,
                pump,
                fast_deadlines(),
            )
            .expect("clean exit with skipped resize must be Ok"),
        );

        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert!(
            state.calls.lock().unwrap().is_empty(),
            "no resize should be applied"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resize_failure_surfaces_and_kills_child() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 1_000_000);
        let handle = child.handle();
        let signals = MockSignalSource::new([vec![SignalEvent::Resize]]);
        let (master, _state) = MockMaster::shared(true, None);

        let err = run_pump_session_with_signals(
            child,
            Box::new(master),
            &signals,
            || Some(pty_size(100, 30)),
            quiet_pump(),
            fast_deadlines(),
        );
        let err = match err {
            Ok(_) => panic!("resize failure must surface"),
            Err(err) => err,
        };

        assert!(matches!(err, Error::Pty(_)), "got {err:?}");
        assert!(
            handle.lock().unwrap().kills >= 1,
            "resize failure must kill the child on unwind"
        );
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_event_returns_deferred_shutdown() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 1_000_000);
        let handle = child.handle();
        let signals = MockSignalSource::new([vec![SignalEvent::Shutdown]]);
        let (master, _state) = MockMaster::shared(false, Some(42_042));

        let status = run_pump_session_with_signals(
            child,
            Box::new(master),
            &signals,
            || Some(pty_size(100, 30)),
            quiet_pump(),
            fast_deadlines(),
        )
        .expect("shutdown event should defer completion");

        match status {
            SessionStatus::Exited(code) => {
                panic!("shutdown event should defer, got immediate exit {code:?}")
            }
            SessionStatus::DeferredShutdown(_) => {}
        }
        assert_eq!(
            handle.lock().unwrap().kills,
            0,
            "deferred shutdown must not trigger the SIGHUP kill guard on return"
        );
    }

    #[cfg(unix)]
    #[test]
    fn deferred_shutdown_rechecks_child_exit_before_forwarding_sigterm() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let handle = child.handle();

        let code = DeferredShutdown {
            child,
            master: Box::new(DummyMaster),
            signal_guard: None,
            host_to_child: None,
            child_to_host: None,
            host_to_child_render_progress: None,
            h2c_result: None,
            c2h_result: None,
            deadlines: fast_deadlines(),
        }
        .complete()
        .expect("already-exited child should bypass SIGTERM forwarding");

        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        assert_eq!(
            handle.lock().unwrap().kills,
            0,
            "already-exited child should not be killed during deferred completion"
        );
    }

    #[cfg(unix)]
    #[test]
    fn deferred_shutdown_resolves_shutdown_target_from_live_master_state() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 1_000_000);
        let (master, state) = MockMaster::shared(false, Some(7));
        let shutdown = DeferredShutdown {
            child,
            master: Box::new(master),
            signal_guard: None,
            host_to_child: None,
            child_to_host: None,
            host_to_child_render_progress: None,
            h2c_result: None,
            c2h_result: None,
            deadlines: fast_deadlines(),
        };

        assert!(matches!(
            shutdown.resolve_shutdown_target(),
            Some(ShutdownTarget::ProcessGroup(7))
        ));
        MockMaster::set_process_group_leader(state.as_ref(), Some(54_054));
        assert!(matches!(
            shutdown.resolve_shutdown_target(),
            Some(ShutdownTarget::ProcessGroup(54_054))
        ));
        MockMaster::set_process_group_leader(state.as_ref(), None);
        assert!(matches!(
            shutdown.resolve_shutdown_target(),
            Some(ShutdownTarget::Process(42_042))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn deferred_shutdown_keeps_signal_guard_installed_until_completion() {
        let _serial = TEST_SERIAL.lock().unwrap();
        let child = MockChild::new(ExitStatus::with_exit_code(0), 0);
        let guard = SignalGuard::install().expect("install signal guard");
        let shutdown = DeferredShutdown {
            child,
            master: Box::new(DummyMaster),
            signal_guard: Some(guard),
            host_to_child: None,
            child_to_host: None,
            host_to_child_render_progress: None,
            h2c_result: None,
            c2h_result: None,
            deadlines: fast_deadlines(),
        };

        let err = SignalGuard::install().expect_err("deferred shutdown should retain the guard");
        assert!(
            matches!(&err, Error::Terminal(io_err) if io_err.kind() == io::ErrorKind::AlreadyExists),
            "expected Terminal(AlreadyExists), got {err:?}"
        );

        let code = shutdown
            .complete()
            .expect("already-exited child should release the retained signal guard cleanly");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));

        let guard = SignalGuard::install().expect("signal guard should drop after completion");
        drop(guard);
    }

    #[cfg(unix)]
    #[test]
    fn deferred_shutdown_kills_child_on_completion_error() {
        let child = MockChild::new(ExitStatus::with_exit_code(0), 1_000_000);
        let handle = child.handle();

        let err = DeferredShutdown {
            child,
            master: Box::new(DummyMaster),
            signal_guard: None,
            host_to_child: None,
            child_to_host: None,
            host_to_child_render_progress: None,
            h2c_result: None,
            c2h_result: None,
            deadlines: fast_deadlines(),
        }
        .complete()
        .expect_err("missing shutdown target must surface");

        assert!(matches!(err, Error::Pty(_)), "got {err:?}");
        assert_eq!(
            handle.lock().unwrap().kills,
            1,
            "completion failure must still best-effort kill the child on drop"
        );
    }

    #[cfg(unix)]
    #[test]
    fn signal_setup_failure_kills_child_before_supervisor_handoff() {
        let state = Arc::new(Mutex::new(MockState {
            polls_remaining: 0,
            scheduled: ExitStatus::with_exit_code(0),
            kills: 0,
            try_waits: 0,
        }));

        let err = run_pump_session_unix_with_setup(
            Box::new(PortableMockChild {
                state: Arc::clone(&state),
            }),
            Box::new(DummyMaster),
            quiet_pump(),
            || Err(io::Error::new(io::ErrorKind::AlreadyExists, "synthetic setup failure").into()),
        );
        let err = match err {
            Ok(_) => panic!("setup failure must surface"),
            Err(err) => err,
        };

        assert!(
            matches!(&err, Error::Terminal(io_err) if io_err.kind() == io::ErrorKind::AlreadyExists),
            "expected Terminal(AlreadyExists), got {err:?}"
        );
        assert_eq!(
            state.lock().unwrap().kills,
            1,
            "outer setup guard must kill the child on setup failure"
        );
    }
}

// Real-PTY integration tests for the supervisor. Mirrors the
// bounded-deadline pattern from `tests/pty_smoke.rs`,
// `src/pty/spawn.rs::real_spawn`, and `src/pty/pump.rs::real_pump`
// (constants duplicated locally per repo convention).
#[cfg(all(test, unix))]
mod real_session {
    use super::*;
    use crate::pty::pump::{start_io_pump, start_io_pump_pollable};
    use crate::pty::spawn::spawn_child;
    use portable_pty::PtySize;
    use std::ffi::{OsStr, OsString};
    use std::io::Cursor;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

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
        let mut deadlines = Deadlines::production();
        deadlines.child_wait_deadline = Some(WAIT_BUDGET);
        deadlines.forwarder_join_budget = JOIN_BUDGET;
        deadlines.wait_poll = WAIT_POLL;
        deadlines.post_kill_wait_budget = WAIT_BUDGET;
        deadlines
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

    #[derive(Clone, Default)]
    struct SharedLog {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedLog {
        fn snapshot(&self) -> String {
            String::from_utf8_lossy(&self.inner.lock().unwrap()).into_owned()
        }
    }

    impl io::Write for SharedLog {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for SharedLog {
        type Writer = SharedLog;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
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
        let pump = start_io_pump(&mut session, Cursor::new(host_stdin), sink.clone(), true)
            .expect("start_io_pump");
        let result = run_pump_session_with(
            PtyChild {
                child: session.child,
            },
            pump,
            supervisor_deadlines(),
        );
        // Keep `master` alive until after the supervisor returns
        // (mirrors the run_pump_session contract that the master
        // must outlive the wait/drain phase) by binding it
        // explicitly here, then dropping it at the end of scope.
        drop(session.master);
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
        let pump = start_io_pump(&mut session, Cursor::new(Vec::<u8>::new()), sink, true)
            .expect("start_io_pump");

        // Signal the child BEFORE handing the session to the
        // supervisor so the very first poll sees a signal
        // status. portable-pty's ChildKiller for unix actually
        // sends SIGHUP (verified in portable-pty 0.9.0
        // src/lib.rs:328), not SIGTERM, so we expect signum=1.
        term.kill().expect("send SIGHUP to child via clone_killer");
        let res = run_pump_session_with(
            PtyChild {
                child: session.child,
            },
            pump,
            supervisor_deadlines(),
        );
        // Keep `master` alive across the supervisor call.
        drop(session.master);
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
    fn supervisor_cancels_pollable_host_stdin_after_signal_death() {
        let mut session =
            spawn_child(OsStr::new("/bin/cat"), &[], pty_size_80x24()).expect("spawn /bin/cat");

        let mut term = session.child.clone_killer();
        let sink = SharedSink::default();
        let (host_stdin, _hold_open) = UnixStream::pair().expect("unix stream pair");
        let host_stdin_fd = host_stdin.as_raw_fd();
        let pump = start_io_pump_pollable(&mut session, host_stdin, sink, true, host_stdin_fd)
            .expect("start pollable pump");

        let logs = SharedLog::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_max_level(tracing::Level::DEBUG)
            .finish();

        let start = Instant::now();
        let res = tracing::subscriber::with_default(subscriber, || {
            term.kill().expect("signal cat via clone_killer");
            run_pump_session_with(
                PtyChild {
                    child: session.child,
                },
                pump,
                supervisor_deadlines(),
            )
        });
        let elapsed = start.elapsed();
        drop(session.master);

        let err = res.expect_err("signal death must surface");
        match err {
            Error::Signal { signum } => assert_eq!(signum, 1),
            other => panic!("expected Signal, got {other:?}"),
        }
        assert!(
            elapsed < JOIN_BUDGET,
            "pollable host stdin cancellation should finish within join budget: {elapsed:?}"
        );
        let captured = logs.snapshot();
        assert!(
            !captured.contains("forwarder join exceeded budget"),
            "host->child forwarder should join instead of timing out: {captured}"
        );
        assert!(
            !captured.contains("zero-budget detach"),
            "host->child forwarder should no longer use zero-budget detach: {captured}"
        );
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
}
