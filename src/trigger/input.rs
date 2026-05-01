//! Input-side trigger pump state machine.
//!
//! `InputPump` is the pure, byte-at-a-time state machine that the
//! real PTY host-to-child pump will call before deciding what to do
//! with user input. It owns the three trigger components that must
//! be consulted together:
//!
//! 1. [`PasteTracker`] watches the user's input stream for
//!    bracketed-paste spans.
//! 2. [`AltScreenTracker`] watches the child output stream, but
//!    its current state disarms user input while a TUI owns the
//!    alternate screen.
//! 3. [`Parser`] classifies normal prompt input as `:q`, `:wq`,
//!    or `:q!`.
//!
//! The type intentionally has no I/O side effects. Phase 3 can
//! wire it in as detect-only logging while still forwarding bytes
//! unchanged; later phases can use the same observations to route
//! matching trigger bytes away from the child.

use super::{
    altscreen::AltScreenTracker,
    parser::{Outcome, Parser},
    paste::PasteTracker,
};

/// Why an input byte bypassed the literal parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BypassReason {
    /// The byte was inside, or completed, a bracketed-paste span.
    Paste,
    /// The child is currently using the alternate screen buffer.
    AltScreen,
}

/// Result of feeding one host-input byte into [`InputPump`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputObservation {
    /// The parser was bypassed because trigger detection is
    /// temporarily disarmed.
    Bypassed(BypassReason),
    /// The parser observed the byte. A non-`None` outcome means
    /// the byte completed a trigger literal.
    Parsed(Outcome),
}

impl InputObservation {
    /// Return the trigger outcome, or [`Outcome::None`] when the
    /// byte was bypassed.
    pub fn outcome(self) -> Outcome {
        match self {
            Self::Bypassed(_) => Outcome::None,
            Self::Parsed(outcome) => outcome,
        }
    }
}

/// Pure state machine for input-side trigger detection.
#[derive(Debug, Default)]
pub struct InputPump {
    paste: PasteTracker,
    alt_screen: AltScreenTracker,
    parser: Parser,
}

impl InputPump {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume one byte from host input and return the trigger
    /// observation for that byte.
    ///
    /// The ordering is the Phase 3 contract: paste tracker first,
    /// current alt-screen state second, parser last. Paste bypass
    /// checks both the pre-byte and post-byte states so marker
    /// terminators such as the trailing `~` in `ESC[201~` never
    /// leak into the parser.
    pub fn feed_input_byte(&mut self, b: u8) -> InputObservation {
        let was_in_paste = self.paste.in_paste();
        let now_in_paste = self.paste.feed(b);
        if was_in_paste || now_in_paste {
            self.parser.reset();
            return InputObservation::Bypassed(BypassReason::Paste);
        }

        if self.alt_screen.is_alt_screen() {
            self.parser.reset();
            return InputObservation::Bypassed(BypassReason::AltScreen);
        }

        InputObservation::Parsed(self.parser.feed(b))
    }

    /// Consume one byte from child output so the input side can
    /// disarm while the alternate screen is active.
    ///
    /// Returns the post-byte alt-screen state. Parser state is
    /// reset on transitions so a partial line cannot survive an
    /// alt-screen window.
    pub fn feed_child_output_byte(&mut self, b: u8) -> bool {
        let was_alt_screen = self.alt_screen.is_alt_screen();
        let now_alt_screen = self.alt_screen.feed(b);
        if was_alt_screen != now_alt_screen {
            self.parser.reset();
        }
        now_alt_screen
    }

    /// Convenience helper for child output chunks.
    pub fn feed_child_output_slice(&mut self, bytes: &[u8]) -> bool {
        for &b in bytes {
            self.feed_child_output_byte(b);
        }
        self.alt_screen.is_alt_screen()
    }

    pub fn in_paste(&self) -> bool {
        self.paste.in_paste()
    }

    pub fn is_alt_screen(&self) -> bool {
        self.alt_screen.is_alt_screen()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEGIN_PASTE: &[u8] = b"\x1b[200~";
    const END_PASTE: &[u8] = b"\x1b[201~";
    const ENTER_ALT: &[u8] = b"\x1b[?1049h";
    const LEAVE_ALT: &[u8] = b"\x1b[?1049l";

    fn outcomes_for_input(pump: &mut InputPump, bytes: &[u8]) -> Vec<Outcome> {
        bytes
            .iter()
            .map(|&b| pump.feed_input_byte(b).outcome())
            .filter(|&outcome| outcome != Outcome::None)
            .collect()
    }

    #[test]
    fn plain_input_reaches_parser() {
        let mut pump = InputPump::new();
        assert_eq!(outcomes_for_input(&mut pump, b":q\n"), vec![Outcome::Q]);
        assert!(!pump.in_paste());
        assert!(!pump.is_alt_screen());
    }

    #[test]
    fn bracketed_paste_bypasses_parser_then_rearms() {
        let mut pump = InputPump::new();
        let mut stream = Vec::new();
        stream.extend_from_slice(BEGIN_PASTE);
        stream.extend_from_slice(b":q\n");
        stream.extend_from_slice(END_PASTE);
        stream.extend_from_slice(b":wq\n");

        assert_eq!(
            outcomes_for_input(&mut pump, &stream),
            vec![Outcome::Wq],
            "trigger text inside paste must be bypassed"
        );
    }

    #[test]
    fn paste_bypass_resets_partial_parser_state() {
        let mut pump = InputPump::new();
        assert_eq!(
            pump.feed_input_byte(b':'),
            InputObservation::Parsed(Outcome::None)
        );

        for &b in BEGIN_PASTE {
            pump.feed_input_byte(b);
        }
        for &b in END_PASTE {
            pump.feed_input_byte(b);
        }

        assert_eq!(outcomes_for_input(&mut pump, b"q\n"), vec![]);
    }

    #[test]
    fn alt_screen_output_disarms_input_until_leave() {
        let mut pump = InputPump::new();
        pump.feed_child_output_slice(ENTER_ALT);
        assert!(pump.is_alt_screen());
        assert_eq!(outcomes_for_input(&mut pump, b":q\n"), vec![]);

        pump.feed_child_output_slice(LEAVE_ALT);
        assert!(!pump.is_alt_screen());
        assert_eq!(
            outcomes_for_input(&mut pump, b":q!\n"),
            vec![Outcome::QBang]
        );
    }

    #[test]
    fn alt_screen_transition_resets_partial_parser_state() {
        let mut pump = InputPump::new();
        assert_eq!(
            pump.feed_input_byte(b':'),
            InputObservation::Parsed(Outcome::None)
        );

        pump.feed_child_output_slice(ENTER_ALT);
        pump.feed_child_output_slice(LEAVE_ALT);

        assert_eq!(outcomes_for_input(&mut pump, b"q\n"), vec![]);
    }

    #[test]
    fn paste_bypass_reports_reason() {
        let mut pump = InputPump::new();
        for &b in &BEGIN_PASTE[..BEGIN_PASTE.len() - 1] {
            assert_eq!(
                pump.feed_input_byte(b),
                InputObservation::Parsed(Outcome::None)
            );
        }
        assert_eq!(
            pump.feed_input_byte(b'~'),
            InputObservation::Bypassed(BypassReason::Paste)
        );
    }

    #[test]
    fn alt_screen_bypass_reports_reason() {
        let mut pump = InputPump::new();
        pump.feed_child_output_slice(ENTER_ALT);
        assert_eq!(
            pump.feed_input_byte(b':'),
            InputObservation::Bypassed(BypassReason::AltScreen)
        );
    }
}
