//! Bracketed-paste tracker.
//!
//! Modern terminals (xterm, iTerm2, Windows Terminal, etc.) wrap
//! pasted text in `\x1b[200~` ... `\x1b[201~` whenever the
//! foreground application has enabled bracketed-paste mode (DEC
//! private mode 2004). While the user is inside such a span, any
//! `:q` / `:wq` / `:q!` literal that happens to appear in the
//! pasted bytes must NOT arm our parser -- the user is pasting
//! source code, not typing a quit command.
//!
//! This tracker is a pure observer. It never mutates, buffers,
//! or steals bytes; the caller keeps forwarding the original
//! stream unchanged to the child PTY and uses the tracker result
//! to decide whether the trigger parser should see each byte. The
//! tracker only answers one question:
//! "after consuming the byte I just gave you, am I currently
//! inside a bracketed-paste span?".
//!
//! State machine (six labelled states, one transition per byte):
//!
//! ```text
//!     Ground -- 0x1B --> Esc
//!     Esc    -- '['  --> Csi
//!     Csi    -- '2'  --> Two
//!     Two    -- '0'  --> TwoZero
//!     TwoZero-- '0'  --> Begin       (then '~' -> in_paste = true)
//!     TwoZero-- '1'  --> End         (then '~' -> in_paste = false)
//!     <any non-matching byte>        -> Ground
//!     <ESC anywhere>                 -> Esc (allows resync)
//! ```
//!
//! The `in_paste` flag flips on the terminating `~`, never on
//! the prefix. ESC anywhere -- including inside a paste span --
//! restarts the recognizer, so a legitimate `\x1b[201~` end
//! marker is still recognized when paste mode is on.
//!
//! ## v0.1 bracketed-paste policy
//!
//! The wrapper is **observer-only** for DEC private mode 2004:
//! it never emits `\x1b[?2004h` / `\x1b[?2004l`, never strips
//! bracketed-paste markers, and never fabricates them. If the
//! child enables bracketed-paste mode, the terminal sends
//! `\x1b[200~` / `\x1b[201~` in the user's input stream; the
//! pump forwards those bytes to the child unchanged while this
//! tracker uses them only to decide whether trigger parsing is
//! temporarily disarmed. If the child does not enable mode 2004,
//! pasted text is indistinguishable from typed text and remains
//! eligible for normal trigger parsing.

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
enum State {
    #[default]
    Ground,
    Esc,
    Csi,
    Two,
    TwoZero,
    /// Saw `\x1b[200`; one `~` away from entering paste.
    Begin,
    /// Saw `\x1b[201`; one `~` away from exiting paste.
    End,
}

/// Stateful byte sink that tracks bracketed-paste mode.
#[derive(Debug, Default)]
pub struct PasteTracker {
    state: State,
    in_paste: bool,
}

impl PasteTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the tracker is currently inside a bracketed paste
    /// span (i.e. between a fully consumed `\x1b[200~` and its
    /// matching `\x1b[201~`).
    pub fn in_paste(&self) -> bool {
        self.in_paste
    }

    /// Consume one byte and return the post-byte paste state.
    ///
    /// **Pump contract:** the simple
    /// `if !tracker.feed(b) { parser.feed(b); }` shortcut is
    /// wrong because it lets the closing `~` of `\x1b[201~` fall
    /// through to the parser and skips the required disarm-boundary
    /// reset. The canonical idiom is "bypass when EITHER the pre-
    /// or post-byte state was active, and reset the parser on every
    /// transition", e.g.:
    ///
    /// ```text
    /// let was_in_paste = tracker.in_paste();
    /// let now_in_paste = tracker.feed(b);
    /// if was_in_paste != now_in_paste {
    ///     parser.reset();
    /// }
    /// if !was_in_paste && !now_in_paste {
    ///     parser.feed(b);
    /// }
    /// ```
    ///
    /// See `tests/trigger_grammar.rs` for a worked reference pump.
    pub fn feed(&mut self, b: u8) -> bool {
        self.state = match (self.state, b) {
            // ESC always restarts the recognizer, even mid-paste,
            // so the end marker is recoverable from any state.
            (_, 0x1b) => State::Esc,
            (State::Esc, b'[') => State::Csi,
            (State::Csi, b'2') => State::Two,
            (State::Two, b'0') => State::TwoZero,
            (State::TwoZero, b'0') => State::Begin,
            (State::TwoZero, b'1') => State::End,
            (State::Begin, b'~') => {
                self.in_paste = true;
                State::Ground
            }
            (State::End, b'~') => {
                self.in_paste = false;
                State::Ground
            }
            // Any other byte breaks the prefix; we drop back to
            // Ground without touching `in_paste`. The byte itself
            // is not "consumed" in the sense of being stolen -- we
            // never own bytes, we only watch them stream by.
            _ => State::Ground,
        };
        self.in_paste
    }

    /// Convenience for tests / future call sites that already
    /// have a slice handy. Equivalent to feeding each byte in
    /// sequence and returning the final `in_paste` state.
    pub fn feed_slice(&mut self, bytes: &[u8]) -> bool {
        for &b in bytes {
            self.feed(b);
        }
        self.in_paste
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEGIN: &[u8] = b"\x1b[200~";
    const END: &[u8] = b"\x1b[201~";

    #[test]
    fn fresh_tracker_is_not_in_paste() {
        let t = PasteTracker::new();
        assert!(!t.in_paste());
    }

    #[test]
    fn complete_begin_marker_enters_paste() {
        let mut t = PasteTracker::new();
        assert!(t.feed_slice(BEGIN));
        assert!(t.in_paste());
    }

    #[test]
    fn complete_end_marker_exits_paste() {
        let mut t = PasteTracker::new();
        t.feed_slice(BEGIN);
        assert!(!t.feed_slice(END));
        assert!(!t.in_paste());
    }

    #[test]
    fn paste_state_does_not_flip_until_terminating_tilde() {
        let mut t = PasteTracker::new();
        // Feed everything except the trailing `~`.
        for &b in &BEGIN[..BEGIN.len() - 1] {
            t.feed(b);
        }
        assert!(!t.in_paste(), "must not enter paste before ~");
        t.feed(b'~');
        assert!(t.in_paste(), "must enter paste on terminating ~");
    }

    #[test]
    fn random_bytes_never_toggle_paste_state() {
        let mut t = PasteTracker::new();
        for b in 0u8..=255 {
            // Skip ESC because it legitimately starts a recognizer.
            if b == 0x1b {
                continue;
            }
            t.feed(b);
            assert!(!t.in_paste(), "byte 0x{b:02x} flipped state");
        }
    }

    #[test]
    fn byte_at_a_time_matches_slice_feed() {
        let stream: Vec<u8> = [b"hello ", BEGIN, b"x:q\n", END, b" tail"].concat();

        let mut a = PasteTracker::new();
        for &b in &stream {
            a.feed(b);
        }

        let mut b = PasteTracker::new();
        b.feed_slice(&stream);

        assert_eq!(a.in_paste(), b.in_paste());
    }

    #[test]
    fn split_begin_marker_across_reads_still_enters_paste() {
        // Cross-read-boundary at every byte position.
        for split in 0..=BEGIN.len() {
            let (left, right) = BEGIN.split_at(split);
            let mut t = PasteTracker::new();
            t.feed_slice(left);
            t.feed_slice(right);
            assert!(t.in_paste(), "split at {split}: paste state not entered");
        }
    }

    #[test]
    fn end_marker_recognized_inside_paste_span() {
        let mut t = PasteTracker::new();
        t.feed_slice(BEGIN);
        // Pasted content that contains a literal ':q' -- must NOT
        // exit paste, must NOT be observable as a trigger here.
        t.feed_slice(b":q\nlots of pasted code\n");
        assert!(t.in_paste());
        t.feed_slice(END);
        assert!(!t.in_paste());
    }

    #[test]
    fn esc_inside_paste_resets_recognizer_for_end_marker() {
        let mut t = PasteTracker::new();
        t.feed_slice(BEGIN);
        // A spurious ESC sequence that does NOT match 201~ should
        // not exit paste.
        t.feed_slice(b"\x1b[Hsomething\x1b[1;1H");
        assert!(t.in_paste());
        // The real end marker still works.
        t.feed_slice(END);
        assert!(!t.in_paste());
    }

    #[test]
    fn broken_prefix_then_real_begin_still_enters_paste() {
        // The user types a near-miss first, then a real begin
        // marker arrives in the same buffer.
        let mut t = PasteTracker::new();
        t.feed_slice(b"\x1b[20foo");
        assert!(!t.in_paste());
        t.feed_slice(BEGIN);
        assert!(t.in_paste());
    }
}
