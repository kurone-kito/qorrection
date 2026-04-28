//! Alt-screen tracker (output side).
//!
//! Full-screen TUIs (vim, less, htop, tmux's copy mode, the
//! Copilot CLI's interactive picker, …) ask the terminal to
//! switch to the alternate screen buffer with `\x1b[?1049h` and
//! restore the primary buffer on exit with `\x1b[?1049l`. While
//! the alternate screen is up, the user is interacting with that
//! TUI, not with the parent shell prompt, so a `:q` typed at vim
//! must not arm our trigger — vim itself owns that keystroke.
//!
//! Unlike the paste tracker (which watches the **input** stream
//! the user types), this tracker watches the **output** stream
//! the child PTY emits, because alt-screen mode is set by the
//! application via terminal-control sequences. The Phase E
//! output arbiter feeds bytes here before forwarding them to the
//! real terminal.
//!
//! We accept all four common alt-screen mode numbers because
//! different terminals and applications historically used
//! different ones interchangeably:
//!
//! - `?1049` — modern xterm: save cursor + switch + clear
//! - `?1047` — older xterm: switch + clear (no save)
//! - `?1048` — save/restore cursor only (no switch)
//! - `?47`   — original xterm: switch only
//!
//! `?1048` does not actually switch buffers, so we ignore it for
//! tracking purposes. The other three all imply alt-screen.
//!
//! Recognizer shape: `\x1b [ ? <digits> h|l`. We refuse to
//! recognize sequences with intermediate parameters or private
//! markers other than the `?` we just consumed, so a stray
//! `\x1b[?1049;1h` won't false-trigger.

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
enum State {
    #[default]
    Ground,
    /// Saw ESC.
    Esc,
    /// Saw `\x1b[`.
    Csi,
    /// Saw `\x1b[?`. Now collecting decimal digits into `param`.
    Param,
}

/// Stateful byte sink that tracks alternate-screen mode.
#[derive(Debug, Default)]
pub struct AltScreenTracker {
    state: State,
    /// Decimal accumulator for the parameter digits in `Param`.
    /// Capped at u32::MAX/10 to prevent overflow on hostile
    /// streams; values that overflow are simply ignored — they
    /// cannot match any of the four mode numbers anyway.
    param: u32,
    on: bool,
}

impl AltScreenTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the alternate screen buffer is currently active
    /// according to the byte stream consumed so far.
    pub fn is_alt_screen(&self) -> bool {
        self.on
    }

    /// Consume one byte from the child's output stream. Returns
    /// the post-byte alt-screen state for convenience.
    pub fn feed(&mut self, b: u8) -> bool {
        match (self.state, b) {
            // ESC always restarts the recognizer, even mid-CSI,
            // so a malformed sequence followed by a real
            // \x1b[?1049h still triggers.
            (_, 0x1b) => {
                self.state = State::Esc;
                self.param = 0;
            }
            (State::Esc, b'[') => {
                self.state = State::Csi;
            }
            (State::Csi, b'?') => {
                self.state = State::Param;
                self.param = 0;
            }
            (State::Param, b'0'..=b'9') => {
                // Saturating-style guard: ignore further digits
                // once we exceed any plausible mode number. The
                // four modes we care about all fit in 5 digits.
                if self.param < 1_000_000 {
                    self.param = self.param * 10 + u32::from(b - b'0');
                }
            }
            (State::Param, b'h') => {
                if matches!(self.param, 47 | 1047 | 1049) {
                    self.on = true;
                }
                self.reset();
            }
            (State::Param, b'l') => {
                if matches!(self.param, 47 | 1047 | 1049) {
                    self.on = false;
                }
                self.reset();
            }
            // Any other byte breaks the prefix; drop back to
            // Ground without touching `on`. Notably `;` (a
            // multi-parameter separator) lands here, so
            // `\x1b[?1049;1h` is rejected — we only honour the
            // single-parameter form.
            _ => self.reset(),
        }
        self.on
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.param = 0;
    }

    /// Slice helper symmetric with the paste tracker.
    pub fn feed_slice(&mut self, bytes: &[u8]) -> bool {
        for &b in bytes {
            self.feed(b);
        }
        self.on
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_tracker_is_on_primary_screen() {
        assert!(!AltScreenTracker::new().is_alt_screen());
    }

    #[test]
    fn enter_and_leave_xterm_1049() {
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1049h");
        assert!(t.is_alt_screen());
        t.feed_slice(b"\x1b[?1049l");
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn enter_and_leave_legacy_47_and_1047() {
        for mode in [b"47".as_ref(), b"1047".as_ref()] {
            let mut t = AltScreenTracker::new();
            let mut on = Vec::from(b"\x1b[?".as_ref());
            on.extend_from_slice(mode);
            on.push(b'h');
            let mut off = Vec::from(b"\x1b[?".as_ref());
            off.extend_from_slice(mode);
            off.push(b'l');

            t.feed_slice(&on);
            assert!(t.is_alt_screen(), "mode {mode:?} did not enter");
            t.feed_slice(&off);
            assert!(!t.is_alt_screen(), "mode {mode:?} did not leave");
        }
    }

    #[test]
    fn mode_1048_does_not_toggle_buffer() {
        // ?1048 saves/restores the cursor only — no buffer swap.
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1048h");
        assert!(!t.is_alt_screen());
        t.feed_slice(b"\x1b[?1048l");
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn split_enter_marker_across_reads_at_every_position() {
        let marker = b"\x1b[?1049h";
        for split in 0..=marker.len() {
            let mut t = AltScreenTracker::new();
            let (a, b) = marker.split_at(split);
            t.feed_slice(a);
            t.feed_slice(b);
            assert!(t.is_alt_screen(), "split at {split}");
        }
    }

    #[test]
    fn multi_param_form_is_rejected() {
        // We only honour the single-parameter form; `;` aborts.
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1049;1h");
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn unrelated_csi_does_not_toggle() {
        let mut t = AltScreenTracker::new();
        // Common cursor / clear sequences.
        t.feed_slice(b"\x1b[H\x1b[2J\x1b[1;1H\x1b[?25l\x1b[?25h");
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn random_non_esc_bytes_never_toggle() {
        let mut t = AltScreenTracker::new();
        for b in 0u8..=255 {
            if b == 0x1b {
                continue;
            }
            t.feed(b);
            assert!(!t.is_alt_screen(), "byte 0x{b:02x} flipped state");
        }
    }

    #[test]
    fn esc_inside_csi_resyncs_to_a_real_alt_screen_enter() {
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?109"); // partial / wrong prefix
        t.feed_slice(b"\x1b[?1049h");
        assert!(t.is_alt_screen());
    }

    #[test]
    fn huge_parameter_does_not_panic_or_match() {
        let mut s = Vec::from(b"\x1b[?".as_ref());
        s.extend(std::iter::repeat(b'9').take(50));
        s.push(b'h');
        let mut t = AltScreenTracker::new();
        t.feed_slice(&s);
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn nested_enter_then_other_csi_then_leave() {
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1049h");
        // While alt-screen is up, the app keeps emitting all
        // sorts of unrelated control sequences.
        t.feed_slice(b"\x1b[1;1H\x1b[2Jvim contents\x1b[?25l");
        assert!(t.is_alt_screen());
        t.feed_slice(b"\x1b[?1049l");
        assert!(!t.is_alt_screen());
    }
}
