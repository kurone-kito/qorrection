//! Alt-screen tracker (output side).
//!
//! Full-screen TUIs (vim, less, htop, tmux's copy mode, the
//! Copilot CLI's interactive picker, …) ask the terminal to
//! switch to the alternate screen buffer with `\x1b[?1049h` and
//! restore the primary buffer on exit with `\x1b[?1049l`. While
//! the alternate screen is up, the user is interacting with that
//! TUI, not with the parent shell prompt, so a `:q` typed at vim
//! must not arm our trigger -- vim itself owns that keystroke.
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
//! - `?1049` -- modern xterm: save cursor + switch + clear
//! - `?1047` -- older xterm: switch + clear (no save)
//! - `?1048` -- save/restore cursor only (no switch)
//! - `?47`   -- original xterm: switch only
//!
//! `?1048` does not actually switch buffers, so we ignore it for
//! tracking purposes. The other three all imply alt-screen.
//!
//! Recognizer shape: `\x1b [ ? <param-list> h|l`, where
//! `<param-list>` is one or more decimal parameters separated by
//! `;`. The state flips if **any** parameter in the list matches
//! one of the alt-screen mode numbers above, so multi-parameter
//! forms like `\x1b[?1049;1h` (mode 1049 + DECCKM) and
//! `\x1b[?1;1049h` are both honoured. Empty params (e.g. the
//! leading `;` in `\x1b[?;1049h`) decode as `0` and never match.

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
    /// Capped at 1_000_000 (well under `u32::MAX / 10`, so the
    /// `param * 10 + digit` step in `feed` cannot overflow) on
    /// hostile streams; further digits past the cap are ignored
    /// -- the four modes we care about are all 5 digits or less,
    /// so anything past the cap cannot match anyway.
    param: u32,
    /// Sticky flag: set when any completed parameter in the
    /// current CSI sequence matches an alt-screen mode number.
    /// Combined with the final parameter at the terminating
    /// `h`/`l` to decide whether to toggle. Cleared by every
    /// recognizer reset (ESC resync, malformed byte, completed
    /// sequence) so it can never leak into a later sequence.
    saw_alt_param: bool,
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
                // Drop any half-collected multi-parameter alt
                // match -- the new ESC starts a fresh sequence
                // that may not even use the `?` private prefix
                // (e.g. CSI `1 h` with `?` omitted), and the
                // stale flag would otherwise toggle on the next
                // `h`/`l`.
                self.saw_alt_param = false;
            }
            (State::Esc, b'[') => {
                self.state = State::Csi;
            }
            (State::Csi, b'?') => {
                self.state = State::Param;
                self.param = 0;
                self.saw_alt_param = false;
            }
            (State::Param, b'0'..=b'9') => {
                // Saturating-style guard: ignore further digits
                // once we exceed any plausible mode number. The
                // four modes we care about all fit in 5 digits.
                if self.param < 1_000_000 {
                    self.param = self.param * 10 + u32::from(b - b'0');
                }
            }
            (State::Param, b';') => {
                // Multi-parameter list: stash whether the param
                // we just finished matches an alt-screen mode,
                // then reset the accumulator and stay in Param
                // for the next sub-parameter.
                if is_alt_mode(self.param) {
                    self.saw_alt_param = true;
                }
                self.param = 0;
            }
            (State::Param, b'h') => {
                if self.saw_alt_param || is_alt_mode(self.param) {
                    self.on = true;
                }
                self.reset();
            }
            (State::Param, b'l') => {
                if self.saw_alt_param || is_alt_mode(self.param) {
                    self.on = false;
                }
                self.reset();
            }
            // Any other byte breaks the prefix; drop back to
            // Ground without touching `on`.
            _ => self.reset(),
        }
        self.on
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.param = 0;
        self.saw_alt_param = false;
    }

    /// Slice helper symmetric with the paste tracker.
    pub fn feed_slice(&mut self, bytes: &[u8]) -> bool {
        for &b in bytes {
            self.feed(b);
        }
        self.on
    }
}

/// `true` when `param` is one of the alt-screen mode numbers
/// we honour. Kept as a free function so the `;` and `h`/`l`
/// arms in `feed` stay short and obviously consistent.
fn is_alt_mode(param: u32) -> bool {
    matches!(param, 47 | 1047 | 1049)
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
        // ?1048 saves/restores the cursor only -- no buffer swap.
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
    fn multi_param_form_with_alt_mode_first_is_recognized() {
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1049;1h");
        assert!(t.is_alt_screen());
    }

    #[test]
    fn multi_param_form_with_alt_mode_last_is_recognized() {
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1;1049h");
        assert!(t.is_alt_screen());
    }

    #[test]
    fn multi_param_form_without_alt_mode_does_not_toggle() {
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1;25h");
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn multi_param_leave_recognized_anywhere_in_list() {
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1049h");
        assert!(t.is_alt_screen());
        t.feed_slice(b"\x1b[?1;1049l");
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn empty_param_decodes_as_zero_and_does_not_match() {
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?;1h");
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn malformed_then_unrelated_h_does_not_falsely_toggle() {
        let mut t = AltScreenTracker::new();
        // Saw `?1049;` then a non-digit, non-`;`, non-`h`/`l`
        // byte aborts the recognizer; the `saw_alt_param` flag
        // must be cleared so the unrelated `\x1b[?1h` below
        // does not inherit it.
        t.feed_slice(b"\x1b[?1049;X");
        t.feed_slice(b"\x1b[?1h");
        assert!(!t.is_alt_screen());
    }

    #[test]
    fn esc_mid_param_clears_pending_alt_match_for_next_csi() {
        // Regression: ESC restarts the parser mid-list, but the
        // next CSI may omit the `?` private prefix entirely
        // (e.g. CSI `1 h`). The half-collected alt match from
        // the interrupted sequence must not leak into that
        // unrelated terminator.
        let mut t = AltScreenTracker::new();
        t.feed_slice(b"\x1b[?1049;");
        t.feed_slice(b"\x1b[1h");
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
