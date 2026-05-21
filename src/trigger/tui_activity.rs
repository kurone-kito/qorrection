//! Output-side "child is repainting its own TUI" heuristic.
//!
//! [`crate::trigger::altscreen::AltScreenTracker`] only flips
//! when the child uses the standard alt-screen mode-set sequences
//! (`\x1b[?1049h`, `\x1b[?1047h`, `\x1b[?47h`). Modern AI CLIs
//! such as Codex CLI render a TUI without entering alt-screen —
//! they repaint the primary screen with cursor positioning and
//! erase-display sequences instead. Overlaying the animation on
//! such a child via `EnterAlternateScreen`/`LeaveAlternateScreen`
//! corrupts the child's screen model for the rest of the
//! session (roadmap [#186] / fix [#188]).
//!
//! This module is a small byte-stream recognizer that flips a
//! sticky "TUI-active" flag for a bounded window after the
//! child emits one of the repaint signals below. The window is
//! measured in observed bytes — not wall-clock time — so unit
//! tests are deterministic and CI hosts cannot race the flag
//! into a stale state.
//!
//! Recognized signals (any one of these flips the flag):
//!
//! - **ED — `\x1b[2J` / `\x1b[3J`**: erase entire (or scrollback)
//!   display. Every TUI calls one of these to repaint the screen.
//!   Bare `\x1b[J` is ED0 (clear from cursor to end of screen)
//!   and is intentionally **not** recognized — it is too common
//!   in normal shell redraw/clear flows and would false-positive
//!   the heuristic on every prompt refresh.
//! - **CUP — any `\x1b[...H` or `\x1b[...f` terminator**: cursor
//!   position. Recognized for every parameter shape, including
//!   the bare cursor-home forms `\x1b[H` / `\x1b[f` and the
//!   fully parameterized `\x1b[<row>;<col>H` / `\x1b[<row>;<col>f`.
//!   Plain shells emit one of these for the prompt occasionally;
//!   TUIs hammer them. The window-based decay keeps false
//!   positives bounded.
//!
//! Deliberately **not** recognized:
//!
//! - `\x1b[?<n>h`-style private-mode sets are covered by the
//!   alt-screen tracker; double-counting would inflate false
//!   positives.
//! - DECSC / DECRC (`\x1b 7` / `\x1b 8`): many shells and prompt
//!   themes emit save/restore-cursor around every prompt
//!   redraw, which would keep `is_tui_active` flipped through
//!   the entire interactive session. The heuristic stays
//!   focused on signals that indicate a *repaint* (clear +
//!   positioning), not generic cursor stack manipulation.
//!
//! The caller (`InputPump`) is also responsible for keeping the
//! tracker in sync with alt-screen entry/exit — feeding signals
//! to this tracker while the standard alt-screen is owned would
//! make the post-exit window outlive the alt-screen transition
//! and gate the animation for ~512 bytes of normal prompt time
//! after a vim-like child returns to the primary screen. See
//! `InputPump::feed_child_output_byte` for the wiring.
//!
//! [#186]: https://github.com/kurone-kito/qorrection/issues/186
//! [#188]: https://github.com/kurone-kito/qorrection/issues/188

/// Number of subsequent non-signalling output bytes after which
/// the tracker considers the child to no longer be in a TUI
/// repaint cycle. ~512 bytes is roughly six 80-column lines of
/// plain text — short enough that a one-off startup signal
/// stops gating the animation as soon as the child settles into
/// a normal prompt, long enough to cover the typical
/// repaint-then-prompt sequence a TUI emits in a single tick.
/// Tune this constant if real-world Codex/Copilot sessions show
/// the heuristic decaying too eagerly between repaints.
const TUI_WINDOW_BYTES: usize = 512;

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
enum State {
    #[default]
    Ground,
    /// Saw ESC.
    Esc,
    /// Saw `\x1b[`.
    Csi,
}

/// Stateful byte sink that flips a sticky "TUI-active" flag for a
/// bounded window after each recognized repaint signal.
#[derive(Debug, Default)]
pub struct TuiActivityTracker {
    state: State,
    /// First numeric parameter of the in-progress CSI sequence,
    /// or `None` if no digit has arrived yet. ED only counts as a
    /// repaint signal when the parameter is explicitly `2` or `3`
    /// (full clear / clear scrollback); bare `\x1b[J` (ED0,
    /// clear-to-end-of-screen) is too common in normal shell
    /// redraw to be useful here.
    csi_param: Option<u32>,
    /// Bytes observed since the last recognized TUI signal
    /// completed. Saturates at `TUI_WINDOW_BYTES` so a long-lived
    /// non-TUI session does not require unbounded arithmetic.
    bytes_since_signal: usize,
    /// Whether any signal has been observed yet. The fresh
    /// tracker reports `is_tui_active = false`, which matches the
    /// "primary-screen shell prompt" prior state.
    saw_any_signal: bool,
}

impl TuiActivityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the child appears to be inside a TUI repaint cycle.
    pub fn is_tui_active(&self) -> bool {
        self.saw_any_signal && self.bytes_since_signal < TUI_WINDOW_BYTES
    }

    /// Reset the recognizer and the decay window.
    ///
    /// Called by the input pump when the standard alt-screen
    /// tracker transitions (entry **or** exit) so a vim/less
    /// session that scribbles cursor positioning while it owns
    /// the alt screen does not leak a stale "TUI-active" flag
    /// into the next ~512 bytes of normal prompt time after it
    /// hands the primary screen back.
    pub fn reset(&mut self) {
        self.state = State::Ground;
        self.csi_param = None;
        self.bytes_since_signal = TUI_WINDOW_BYTES;
        self.saw_any_signal = false;
    }

    /// Consume one byte from the child's output stream.
    pub fn feed(&mut self, b: u8) {
        // Count every observed byte against the decay window. A
        // recognized signal resets the counter to 0 inside
        // `mark_active`, so the window measures distance to the
        // last signal regardless of how the bytes are split
        // across reads.
        self.bytes_since_signal = self
            .bytes_since_signal
            .saturating_add(1)
            .min(TUI_WINDOW_BYTES);

        match (self.state, b) {
            // ESC always restarts the recognizer so a malformed
            // sequence followed by a real `\x1b[2J` still flips
            // the flag.
            (_, 0x1b) => {
                self.state = State::Esc;
                self.csi_param = None;
            }
            (State::Esc, b'[') => {
                self.state = State::Csi;
                self.csi_param = None;
            }
            (State::Esc, _) => self.state = State::Ground,
            // CSI parameter bytes — accumulate the first numeric
            // parameter so ED's variant can be distinguished from
            // bare ED0.
            (State::Csi, b @ b'0'..=b'9') => {
                let digit = u32::from(b - b'0');
                self.csi_param = Some(
                    self.csi_param
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit),
                );
            }
            (State::Csi, b';') => {
                // Sub-parameter separator. ED only takes one
                // parameter; once a `;` appears we stop counting
                // the first parameter and let the sequence finish
                // on whatever terminator follows. Setting the
                // accumulator to a value outside `{2, 3}` keeps
                // ED-2/3 from false-firing on a multi-parameter
                // sequence that happens to start with `2`.
                self.csi_param = Some(u32::MAX);
            }
            // CUP — cursor position. Both `H` and `f` are CUP
            // terminators per ECMA-48.
            (State::Csi, b'H' | b'f') => {
                self.mark_active();
                self.state = State::Ground;
                self.csi_param = None;
            }
            // ED — erase display. Only treat full-screen
            // variants as TUI signals: `\x1b[2J` (full clear) and
            // `\x1b[3J` (clear scrollback). Bare `\x1b[J` (ED0,
            // clear from cursor to end of screen) is too common
            // in normal shell redraw and is intentionally
            // ignored.
            (State::Csi, b'J') => {
                if matches!(self.csi_param, Some(2) | Some(3)) {
                    self.mark_active();
                }
                self.state = State::Ground;
                self.csi_param = None;
            }
            // `?` private-prefixed sequences are owned by the
            // alt-screen tracker; do not double-count them here.
            (State::Csi, b'?') => {
                self.state = State::Ground;
                self.csi_param = None;
            }
            // Any other CSI terminator: not a TUI signal we
            // track. Drop back to Ground.
            (State::Csi, _) => {
                self.state = State::Ground;
                self.csi_param = None;
            }
            _ => self.state = State::Ground,
        }
    }

    /// Slice helper symmetric with the alt-screen tracker.
    pub fn feed_slice(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.feed(b);
        }
    }

    fn mark_active(&mut self) {
        self.bytes_since_signal = 0;
        self.saw_any_signal = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_tracker_is_not_tui_active() {
        let t = TuiActivityTracker::new();
        assert!(!t.is_tui_active());
    }

    #[test]
    fn ed_2j_flips_to_tui_active() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[2J");
        assert!(t.is_tui_active());
    }

    #[test]
    fn ed_3j_flips_to_tui_active() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[3J");
        assert!(t.is_tui_active());
    }

    #[test]
    fn bare_ed_j_does_not_flip() {
        // ED0 (clear from cursor to end of screen) is too common
        // in normal shell redraw to be a useful TUI signal.
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[J");
        assert!(!t.is_tui_active());
    }

    #[test]
    fn ed_1j_does_not_flip() {
        // ED1 (clear from start of screen to cursor) is not a
        // full-screen repaint and is not recognized.
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[1J");
        assert!(!t.is_tui_active());
    }

    #[test]
    fn cup_with_params_flips_to_tui_active() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[10;20H");
        assert!(t.is_tui_active());
    }

    #[test]
    fn cup_with_lowercase_f_terminator_flips_to_tui_active() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[1;1f");
        assert!(t.is_tui_active());
    }

    #[test]
    fn decsc_does_not_flip() {
        // DECSC `ESC 7` is emitted by many shell prompts around
        // every redraw; keeping it as a TUI signal would gate the
        // animation through the entire interactive session.
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b7");
        assert!(!t.is_tui_active());
    }

    #[test]
    fn decrc_does_not_flip() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b8");
        assert!(!t.is_tui_active());
    }

    #[test]
    fn reset_clears_active_flag() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[2J");
        assert!(t.is_tui_active());
        t.reset();
        assert!(!t.is_tui_active(), "reset must clear the active flag");
    }

    #[test]
    fn alt_screen_private_mode_set_is_ignored() {
        // `?1049h` belongs to the alt-screen tracker; this tracker
        // must not flip on it or false-positive doubles will lock
        // the gag suppression after every alt-screen entry.
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[?1049h");
        assert!(!t.is_tui_active());
    }

    #[test]
    fn cursor_hide_show_private_modes_are_ignored() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[?25l\x1b[?25h");
        assert!(!t.is_tui_active());
    }

    #[test]
    fn plain_text_does_not_flip() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"hello world\nthis is plain stdout\n");
        assert!(!t.is_tui_active());
    }

    #[test]
    fn window_decays_after_n_plain_bytes() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[2J");
        assert!(t.is_tui_active());

        // Feed exactly TUI_WINDOW_BYTES of plain content; the
        // last plain byte counts as "since signal" too, so on the
        // 512th plain byte the window has expired.
        for _ in 0..TUI_WINDOW_BYTES {
            t.feed(b'x');
        }
        assert!(!t.is_tui_active());
    }

    #[test]
    fn window_resets_on_subsequent_signal() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[2J");
        // Bleed off some of the window.
        for _ in 0..(TUI_WINDOW_BYTES / 2) {
            t.feed(b'x');
        }
        assert!(t.is_tui_active());
        // A second signal restarts the counter.
        t.feed_slice(b"\x1b[H");
        for _ in 0..(TUI_WINDOW_BYTES - 1) {
            t.feed(b'x');
        }
        assert!(t.is_tui_active(), "window should reset on a new signal");
    }

    #[test]
    fn esc_mid_csi_restarts_recognizer_to_a_real_signal() {
        let mut t = TuiActivityTracker::new();
        // Start a malformed CSI then interrupt with a real
        // ESC-CSI-J sequence.
        t.feed_slice(b"\x1b[10");
        t.feed_slice(b"\x1b[2J");
        assert!(t.is_tui_active());
    }

    #[test]
    fn split_signal_across_feeds_at_every_position() {
        let marker = b"\x1b[2J";
        for split in 0..=marker.len() {
            let mut t = TuiActivityTracker::new();
            let (a, b) = marker.split_at(split);
            t.feed_slice(a);
            t.feed_slice(b);
            assert!(t.is_tui_active(), "split at {split} did not flip");
        }
    }

    #[test]
    fn unknown_csi_terminator_does_not_flip() {
        // SGR (color) is a CSI but not a repaint signal.
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[31m\x1b[0m");
        assert!(!t.is_tui_active());
    }
}
