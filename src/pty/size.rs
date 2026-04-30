//! Initial PTY size derivation.
//!
//! [`initial_size`] snapshots the host terminal's current size
//! via `crossterm::terminal::size()` and returns a
//! [`PtySize`](portable_pty::PtySize). When the query fails or
//! reports a degenerate `(0, 0)` (occasionally seen on headless
//! environments where the call returns `Ok` without a real
//! terminal attached), the function falls back to the canonical
//! VT100 default of 80×24 so the spawned child always sees a
//! usable size.
//!
//! The pure seam [`initial_size_with`] lets unit tests cover
//! every branch without touching the real terminal.
//!
//! Wired into the wrap session body in PR 5 (#26); kept
//! `pub(crate)` and `dead_code`-allowed in PR 2.

use portable_pty::PtySize;

/// Default fallback size (canonical VT100). Public-to-module so
/// unit tests can compare against the same constant the
/// production path uses.
pub(crate) const FALLBACK_SIZE: PtySize = PtySize {
    cols: 80,
    rows: 24,
    pixel_width: 0,
    pixel_height: 0,
};

/// Snapshot the host terminal's current size for a freshly
/// spawned PTY child.
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) fn initial_size() -> PtySize {
    initial_size_with(crossterm::terminal::size)
}

/// Pure variant of [`initial_size`]: derives the size from an
/// injected `query` instead of the real `crossterm` call. Used
/// directly by unit tests; production code calls [`initial_size`].
pub(crate) fn initial_size_with<F>(query: F) -> PtySize
where
    F: FnOnce() -> std::io::Result<(u16, u16)>,
{
    match query() {
        Ok((cols, rows)) if cols > 0 && rows > 0 => PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        },
        _ => FALLBACK_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn initial_size_uses_query_result_when_positive() {
        let size = initial_size_with(|| Ok((132, 50)));
        assert_eq!(size.cols, 132);
        assert_eq!(size.rows, 50);
        assert_eq!(size.pixel_width, 0);
        assert_eq!(size.pixel_height, 0);
    }

    #[test]
    fn initial_size_falls_back_on_query_err() {
        let size = initial_size_with(|| Err(io::Error::other("no terminal")));
        assert_eq!(size.cols, FALLBACK_SIZE.cols);
        assert_eq!(size.rows, FALLBACK_SIZE.rows);
    }

    #[test]
    fn initial_size_falls_back_when_query_returns_zero() {
        // Headless-environment defence: some CI runners report
        // Ok((0, 0)) instead of Err when no controlling tty is
        // attached. Treat as "no usable size" and fall back.
        for pair in [(0, 0), (0, 24), (80, 0)] {
            let size = initial_size_with(move || Ok(pair));
            assert_eq!(
                (size.cols, size.rows),
                (FALLBACK_SIZE.cols, FALLBACK_SIZE.rows),
                "expected fallback for query={pair:?}"
            );
        }
    }
}
