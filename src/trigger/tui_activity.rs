//! Output-side "child is repainting its own TUI" heuristic.
//!
//! [`AltScreenTracker`] (this module's sibling at
//! [`crate::trigger::altscreen`]) only flips when the child uses
//! the four standard alt-screen mode-set sequences
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
//!   display. Every TUI calls this to repaint the screen.
//! - **CUP — `\x1b[<row>;<col>H` / `\x1b[<row>;<col>f`**: cursor
//!   position. Plain shells emit this for the prompt occasionally;
//!   TUIs hammer it. The window-based decay keeps false positives
//!   bounded.
//! - **DECSC / DECRC — `\x1b 7` / `\x1b 8`**: save / restore
//!   cursor. Often paired with cursor positioning by full-screen
//!   apps.
//!
//! The alt-screen tracker covers `\x1b[?<n>h`-style private-mode
//! sets including the alt-screen variants, so this tracker
//! intentionally does **not** observe `?` private-mode sequences
//! at all — the alt-screen tracker is the authoritative source
//! for them, and double-counting would inflate false positives.
//!
//! [#186]: https://github.com/kurone-kito/qorrection/issues/186
//! [#188]: https://github.com/kurone-kito/qorrection/issues/188

/// Number of subsequent non-signalling output bytes after which
/// the tracker considers the child to no longer be in a TUI
/// repaint cycle. Roughly one screen's worth of plain text on a
/// typical 80-column terminal; tuned conservatively so a single
/// stale repaint signal does not lock the gag suppression on
/// for the rest of the session.
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
            (_, 0x1b) => self.state = State::Esc,
            // DECSC / DECRC fire on the byte after a bare `ESC`.
            (State::Esc, b'7') | (State::Esc, b'8') => {
                self.mark_active();
                self.state = State::Ground;
            }
            (State::Esc, b'[') => self.state = State::Csi,
            (State::Esc, _) => self.state = State::Ground,
            // CSI parameter bytes — keep collecting until a final
            // byte arrives.
            (State::Csi, b'0'..=b'9' | b';') => {}
            // CUP — cursor position. Both `H` and `f` are CUP
            // terminators per ECMA-48.
            (State::Csi, b'H' | b'f') => {
                self.mark_active();
                self.state = State::Ground;
            }
            // ED — erase display. Catches `\x1b[2J`, `\x1b[3J`,
            // and bare `\x1b[J`; all of them are repaint signals.
            (State::Csi, b'J') => {
                self.mark_active();
                self.state = State::Ground;
            }
            // `?` private-prefixed sequences are owned by the
            // alt-screen tracker; do not double-count them here.
            (State::Csi, b'?') => self.state = State::Ground,
            // Any other CSI terminator: not a TUI signal we
            // track. Drop back to Ground.
            (State::Csi, _) => self.state = State::Ground,
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
    fn bare_ed_j_flips_to_tui_active() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b[J");
        assert!(t.is_tui_active());
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
    fn decsc_flips_to_tui_active() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b7");
        assert!(t.is_tui_active());
    }

    #[test]
    fn decrc_flips_to_tui_active() {
        let mut t = TuiActivityTracker::new();
        t.feed_slice(b"\x1b8");
        assert!(t.is_tui_active());
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
