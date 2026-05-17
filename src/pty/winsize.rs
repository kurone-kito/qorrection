//! Windows PTY resize poller.
//!
//! On Windows there is no `SIGWINCH`. This module polls
//! `crossterm::terminal::size()` every 250 ms and sets an
//! atomic flag when the terminal dimensions change. The session
//! supervisor reads the flag on each tick and forwards a resize
//! to the child PTY when set.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Polls the terminal size every 250 ms on a background thread
/// and signals the caller via an atomic flag when the size changes.
///
/// The flag is edge-triggered: [`take_resize`] returns `true`
/// once after each size change and then clears until the next.
///
/// Drop stops the background thread within one poll interval.
///
/// [`take_resize`]: WindowsResizePoller::take_resize
pub(crate) struct WindowsResizePoller {
    resized: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WindowsResizePoller {
    const POLL_INTERVAL: Duration = Duration::from_millis(250);

    /// Start the background polling thread.
    pub(crate) fn start() -> Self {
        let resized = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let resized_t = Arc::clone(&resized);
        let stop_t = Arc::clone(&stop);

        let thread = std::thread::spawn(move || {
            let mut last = crossterm::terminal::size().ok();
            while !stop_t.load(Ordering::Acquire) {
                std::thread::sleep(Self::POLL_INTERVAL);
                if stop_t.load(Ordering::Acquire) {
                    break;
                }
                let now = crossterm::terminal::size().ok();
                if now != last {
                    last = now;
                    resized_t.store(true, Ordering::Release);
                }
            }
        });

        Self {
            resized,
            stop,
            thread: Some(thread),
        }
    }

    /// Returns `true` if the terminal was resized since the last
    /// call. Atomically clears the flag so each resize fires once.
    pub(crate) fn take_resize(&self) -> bool {
        self.resized.swap(false, Ordering::AcqRel)
    }
}

impl Drop for WindowsResizePoller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Detach — the thread observes the stop flag within one
        // poll interval and exits cleanly without a join.
        if let Some(t) = self.thread.take() {
            drop(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn starts_without_pending_resize() {
        let poller = WindowsResizePoller::start();
        assert!(
            !poller.take_resize(),
            "no resize expected immediately after construction"
        );
    }

    #[test]
    fn take_resize_clears_flag() {
        let poller = WindowsResizePoller::start();
        poller.resized.store(true, Ordering::Release);
        assert!(poller.take_resize(), "flag should be set");
        assert!(!poller.take_resize(), "flag should clear after take");
    }

    #[test]
    fn drops_within_poll_interval() {
        let start = Instant::now();
        let poller = WindowsResizePoller::start();
        drop(poller);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "drop should not block: {:?}",
            start.elapsed()
        );
    }
}
