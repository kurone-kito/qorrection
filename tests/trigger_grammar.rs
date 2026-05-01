//! Phase D4 -- exhaustive grammar + boundary test sweep against
//! the public trigger pipeline API.
//!
//! Unit tests in `src/trigger/*` cover each module in isolation.
//! These integration tests pin the *combined* contract exposed by
//! `trigger::input::InputPump`: paste tracker AND alt-screen
//! tracker AND parser, all chained behind the public byte-at-a-time
//! pump API.
//!
//! Pump model under test (minus the actual PTY I/O):
//!
//! ```text
//!     for each input byte b:
//!         observation = pump.feed_input_byte(b)
//!         record(observation.outcome())
//!         pump.feed_child_output_byte(b)   // output-side proxy
//! ```
//!
//! Even though the alt-screen tracker really watches the
//! *output* stream in production, feeding it the same byte
//! sequence here is a sound proxy: any byte that toggles
//! alt-screen would (by the rules of the protocol) appear on
//! output too, and the disarm semantics we care about are the
//! same on both sides. The proxy feeds output after input so the
//! final byte of an alt-screen leave sequence is still bypassed
//! by the pre-byte alt-screen state.

use qorrection::trigger::{input::InputPump, parser::Outcome};

/// Run a stream through the chained pipeline and return the
/// list of non-`None` outcomes the parser emitted.
fn run(stream: &[u8]) -> Vec<Outcome> {
    let mut pump = InputPump::new();
    let mut out = Vec::new();
    for &b in stream {
        let o = pump.feed_input_byte(b).outcome();
        if o != Outcome::None {
            out.push(o);
        }
        pump.feed_child_output_byte(b);
    }
    out
}

#[test]
fn plain_quit_literals_match() {
    assert_eq!(run(b":q\n"), vec![Outcome::Q]);
    assert_eq!(run(b":wq\n"), vec![Outcome::Wq]);
    assert_eq!(run(b":q!\n"), vec![Outcome::QBang]);
}

#[test]
fn pasted_quit_literal_does_not_arm() {
    // The user pastes a code blob that contains `:q\n`; the
    // bracketed-paste tracker must keep us disarmed.
    let stream = b"\x1b[200~:q\n:wq\n:q!\n\x1b[201~";
    assert_eq!(run(stream), vec![]);
}

#[test]
fn pasted_then_typed_quit_still_matches_after_paste() {
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b[200~");
    stream.extend_from_slice(b":q inside paste\n");
    stream.extend_from_slice(b"\x1b[201~");
    stream.extend_from_slice(b":q\n");
    assert_eq!(run(&stream), vec![Outcome::Q]);
}

#[test]
fn unmarked_paste_like_quit_literal_is_plain_input() {
    // v0.1 policy: qorrection does not enable bracketed-paste
    // mode on its own. Without terminal-provided 200~/201~
    // markers, pasted text and typed text are intentionally
    // indistinguishable to the trigger pipeline.
    assert_eq!(run(b":q\n"), vec![Outcome::Q]);
}

#[test]
fn alt_screen_window_disarms_then_rearms() {
    // Enter alt-screen (TUI takes over), user types `:q` inside
    // the TUI (it belongs to the TUI, not us), TUI exits, user
    // then quits the wrapper for real.
    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b[?1049h");
    stream.extend_from_slice(b":q\n:wq\n"); // belongs to TUI
    stream.extend_from_slice(b"\x1b[?1049l");
    stream.extend_from_slice(b":q\n");
    assert_eq!(run(&stream), vec![Outcome::Q]);
}

#[test]
fn dirty_line_followed_by_clean_line_yields_one_match() {
    assert_eq!(run(b":qX\x08q\n:q\n"), vec![Outcome::Q]);
}

#[test]
fn three_back_to_back_distinct_triggers() {
    assert_eq!(
        run(b":q\n:wq\n:q!\n"),
        vec![Outcome::Q, Outcome::Wq, Outcome::QBang]
    );
}

#[test]
fn crlf_terminator_fires_once_per_literal() {
    assert_eq!(
        run(b":q\r\n:wq\r\n:q!\r\n"),
        vec![Outcome::Q, Outcome::Wq, Outcome::QBang]
    );
}

#[test]
fn leading_whitespace_combinations_match() {
    for prefix in [b"   ".as_ref(), b"\t\t".as_ref(), b" \t \t".as_ref()] {
        let mut s = Vec::from(prefix);
        s.extend_from_slice(b":wq\n");
        assert_eq!(run(&s), vec![Outcome::Wq], "prefix={prefix:?}");
    }
}

#[test]
fn trailing_garbage_never_matches() {
    for s in [
        b":q foo\n".as_ref(),
        b":qq\n".as_ref(),
        b":wqx\n".as_ref(),
        b":q!!\n".as_ref(),
    ] {
        assert_eq!(run(s), vec![], "stream={s:?}");
    }
}

#[test]
fn case_variants_never_match() {
    for s in [
        b":Q\n".as_ref(),
        b":WQ\n".as_ref(),
        b":Q!\n".as_ref(),
        b":qQ\n".as_ref(),
    ] {
        assert_eq!(run(s), vec![], "stream={s:?}");
    }
}
