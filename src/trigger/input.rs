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

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

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

/// Shared input-pump state used by the host-input and child-output
/// pump halves.
pub type SharedInputPump = Arc<Mutex<InputPump>>;

pub fn shared_input_pump() -> SharedInputPump {
    Arc::new(Mutex::new(InputPump::new()))
}

/// Host-input [`Write`] adapter for Phase 3 detect-only wiring.
///
/// The adapter preserves the byte stream exactly as accepted by the
/// wrapped writer. It only mirrors accepted host-input bytes into the
/// shared [`InputPump`] and emits an `info` diagnostic when a trigger
/// literal is detected.
#[derive(Debug)]
pub struct InputDetector<W> {
    inner: W,
    input: SharedInputPump,
}

impl<W> InputDetector<W> {
    pub fn new(inner: W, input: SharedInputPump) -> Self {
        Self { inner, input }
    }

    #[cfg(test)]
    fn inner(&self) -> &W {
        &self.inner
    }
}

impl<W> Write for InputDetector<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        if written > 0 {
            if let Err(err) = observe_detected_input(&self.input, &buf[..written], |outcome| {
                tracing::info!(?outcome, "trigger detect-only: matched host input trigger");
            }) {
                tracing::warn!(error = %err, "trigger detect-only observation failed");
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn observe_detected_input<F>(
    input: &SharedInputPump,
    bytes: &[u8],
    mut on_detect: F,
) -> io::Result<()>
where
    F: FnMut(Outcome),
{
    let mut detected = Vec::new();
    let mut input = input.lock().map_err(|_| {
        io::Error::other("trigger input pump mutex poisoned while observing host input")
    })?;
    for &b in bytes {
        let outcome = input.feed_input_byte(b).outcome();
        if outcome != Outcome::None {
            detected.push(outcome);
        }
    }
    drop(input);

    for outcome in detected {
        on_detect(outcome);
    }
    Ok(())
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

    #[derive(Debug, Default)]
    struct OneByteWriter {
        bytes: Vec<u8>,
    }

    impl Write for OneByteWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            self.bytes.push(buf[0]);
            Ok(1)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct BrokenPipeAfterOne {
        bytes: Vec<u8>,
    }

    impl Write for BrokenPipeAfterOne {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            if self.bytes.is_empty() {
                self.bytes.push(buf[0]);
                return Ok(1);
            }
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum PumpStep {
        Host(&'static [u8]),
        Child(&'static [u8]),
    }

    struct PumpCase {
        name: &'static str,
        steps: &'static [PumpStep],
        expected_outcomes: &'static [Outcome],
        final_in_paste: bool,
        final_alt_screen: bool,
    }

    fn outcomes_for_input(pump: &mut InputPump, bytes: &[u8]) -> Vec<Outcome> {
        bytes
            .iter()
            .map(|&b| pump.feed_input_byte(b).outcome())
            .filter(|&outcome| outcome != Outcome::None)
            .collect()
    }

    fn outcomes_for_steps(pump: &mut InputPump, steps: &[PumpStep]) -> Vec<Outcome> {
        let mut outcomes = Vec::new();
        for step in steps {
            match step {
                PumpStep::Host(bytes) => outcomes.extend(outcomes_for_input(pump, bytes)),
                PumpStep::Child(bytes) => {
                    pump.feed_child_output_slice(bytes);
                }
            }
        }
        outcomes
    }

    fn detect_for_test(input: &SharedInputPump, bytes: &[u8]) -> Vec<Outcome> {
        let mut detected = Vec::new();
        observe_detected_input(input, bytes, |outcome| detected.push(outcome)).unwrap();
        detected
    }

    #[test]
    fn detect_only_helper_reports_plain_trigger() {
        let input = shared_input_pump();
        let detected = detect_for_test(&input, b":wq\n");

        assert_eq!(detected, vec![Outcome::Wq]);
    }

    #[test]
    fn detect_only_helper_respects_alt_screen_state() {
        let input = shared_input_pump();
        input.lock().unwrap().feed_child_output_slice(ENTER_ALT);

        assert_eq!(detect_for_test(&input, b":q\n"), vec![]);

        input.lock().unwrap().feed_child_output_slice(LEAVE_ALT);
        assert_eq!(detect_for_test(&input, b":q!\n"), vec![Outcome::QBang]);
    }

    #[test]
    fn input_detector_forwards_bytes_unchanged() {
        let input = shared_input_pump();
        let mut detector = InputDetector::new(Vec::new(), input);

        detector.write_all(b":q\n").unwrap();

        assert_eq!(detector.inner().as_slice(), b":q\n");
    }

    #[test]
    fn input_detector_preserves_cross_write_state() {
        let input = shared_input_pump();
        let mut detector = InputDetector::new(Vec::new(), input.clone());

        assert_eq!(detector.write(b":").unwrap(), 1);
        assert_eq!(detector.write(b"q").unwrap(), 1);
        assert_eq!(detector.write(b"\n").unwrap(), 1);

        assert_eq!(detect_for_test(&input, b":wq\n"), vec![Outcome::Wq]);
    }

    #[test]
    fn input_detector_observes_into_shared_pump() {
        let input = shared_input_pump();
        let mut detector = InputDetector::new(Vec::new(), input.clone());

        detector.write_all(b":").unwrap();

        assert_eq!(detector.inner().as_slice(), b":");
        assert_eq!(detect_for_test(&input, b"q\n"), vec![Outcome::Q]);
    }

    #[test]
    fn input_detector_observes_only_accepted_write_prefix() {
        let input = shared_input_pump();
        let mut detector = InputDetector::new(OneByteWriter::default(), input.clone());

        assert_eq!(detector.write(b":q").unwrap(), 1);

        assert_eq!(detector.inner().bytes.as_slice(), b":");
        assert_eq!(detect_for_test(&input, b"\n"), vec![]);
    }

    #[test]
    fn input_detector_skips_broken_pipe_suffix() {
        let input = shared_input_pump();
        let mut detector = InputDetector::new(BrokenPipeAfterOne::default(), input.clone());

        let err = detector.write_all(b":q").unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(detector.inner().bytes.as_slice(), b":");
        assert_eq!(detect_for_test(&input, b"\n"), vec![]);
    }

    #[test]
    fn observe_detected_input_releases_lock_before_callback() {
        let input = shared_input_pump();
        let mut detected = Vec::new();

        observe_detected_input(&input, b":q\n", |outcome| {
            detected.push(outcome);
            let _guard = input
                .try_lock()
                .expect("input mutex should be released before detection callback");
        })
        .unwrap();

        assert_eq!(detected, vec![Outcome::Q]);
    }

    #[test]
    fn input_detector_keeps_forwarding_when_observer_fails() {
        let input = shared_input_pump();
        let _ = std::panic::catch_unwind({
            let input = input.clone();
            move || {
                let _guard = input.lock().unwrap();
                panic!("poison trigger input mutex for test");
            }
        });

        let mut detector = InputDetector::new(Vec::new(), input);

        detector.write_all(b":q\n").unwrap();

        assert_eq!(detector.inner().as_slice(), b":q\n");
    }

    #[test]
    fn plain_input_reaches_parser() {
        let mut pump = InputPump::new();
        assert_eq!(outcomes_for_input(&mut pump, b":q\n"), vec![Outcome::Q]);
        assert!(!pump.in_paste());
        assert!(!pump.is_alt_screen());
    }

    #[test]
    fn pump_input_modes_follow_table() {
        let cases = [
            PumpCase {
                name: "normal prompt input arms all quit literals",
                steps: &[PumpStep::Host(b":q\n:wq\n:q!\n")],
                expected_outcomes: &[Outcome::Q, Outcome::Wq, Outcome::QBang],
                final_in_paste: false,
                final_alt_screen: false,
            },
            PumpCase {
                name: "bracketed paste bypasses triggers then rearms",
                steps: &[
                    PumpStep::Host(BEGIN_PASTE),
                    PumpStep::Host(b":q\n:wq\n:q!\n"),
                    PumpStep::Host(END_PASTE),
                    PumpStep::Host(b":wq\n"),
                ],
                expected_outcomes: &[Outcome::Wq],
                final_in_paste: false,
                final_alt_screen: false,
            },
            PumpCase {
                name: "alternate screen bypasses triggers then rearms",
                steps: &[
                    PumpStep::Child(ENTER_ALT),
                    PumpStep::Host(b":q\n:wq\n:q!\n"),
                    PumpStep::Child(LEAVE_ALT),
                    PumpStep::Host(b":q!\n"),
                ],
                expected_outcomes: &[Outcome::QBang],
                final_in_paste: false,
                final_alt_screen: false,
            },
        ];

        for case in cases {
            let mut pump = InputPump::new();
            assert_eq!(
                outcomes_for_steps(&mut pump, case.steps),
                case.expected_outcomes,
                "{}",
                case.name
            );
            assert_eq!(pump.in_paste(), case.final_in_paste, "{}", case.name);
            assert_eq!(pump.is_alt_screen(), case.final_alt_screen, "{}", case.name);
        }
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
