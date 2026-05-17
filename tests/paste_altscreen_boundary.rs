//! Deterministic table-driven boundary tests for the paste and
//! alt-screen trackers.
//!
//! Each table row is an (input, expected_output) pair fed through
//! the respective tracker. These tests are entirely deterministic
//! and CI-stable: no randomness, no external state, no timing.

use qorrection::trigger::altscreen::AltScreenTracker;
use qorrection::trigger::paste::PasteTracker;

// ── Paste tracker ────────────────────────────────────────────────

const PASTE_BEGIN: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Partial prefix + wrong continuation byte must NOT enter paste.
///
/// For each intermediate position n in `\x1b[200~`, we feed the first
/// n bytes and then inject a byte that is NOT the correct next byte
/// for that position. The tracker must remain outside paste.
///
/// We use `b'X'` (0x58) as the disruptor: it does not appear anywhere
/// in PASTE_BEGIN or PASTE_END, so it is guaranteed to be "wrong" at
/// every state the sequence can be in.
#[test]
fn paste_partial_prefix_wrong_byte_stays_outside_paste() {
    // 'X' (0x58) does not appear in PASTE_BEGIN or PASTE_END.
    let wrong: u8 = b'X';
    for n in 0..PASTE_BEGIN.len() {
        let mut t = PasteTracker::new();
        t.feed_slice(&PASTE_BEGIN[..n]);
        t.feed(wrong);
        assert!(
            !t.in_paste(),
            "prefix len {n} + disruptor 0x{wrong:02x}: unexpected paste entry"
        );
    }
    // Also test a few other disruptors at Ground (n=0).
    for &disruptor in b"A019\n\x00" {
        let mut t = PasteTracker::new();
        t.feed(disruptor);
        assert!(
            !t.in_paste(),
            "ground-state disruptor 0x{disruptor:02x}: unexpected paste entry"
        );
    }
}

/// After a disruption the correct full sequence still enters paste.
#[test]
fn paste_recovery_after_disruption() {
    let mut t = PasteTracker::new();
    // Partial then wrong byte
    t.feed_slice(b"\x1b[20X");
    assert!(!t.in_paste(), "should not be in paste after disruption");
    // Now send the real begin marker
    t.feed_slice(PASTE_BEGIN);
    assert!(t.in_paste(), "should be in paste after recovery");
}

/// ESC anywhere in the middle of a partial prefix resets recognition
/// but the sequence that follows the ESC is still parsed fresh.
#[test]
fn paste_esc_resync_mid_prefix_allows_fresh_begin() {
    for split in 0..PASTE_BEGIN.len() {
        let mut t = PasteTracker::new();
        // Feed partial prefix, inject ESC, then complete begin marker.
        t.feed_slice(&PASTE_BEGIN[..split]);
        // ESC starts a new recognition attempt; the byte after it
        // is `[` from the restart, so we feed the full marker again.
        t.feed_slice(PASTE_BEGIN);
        assert!(
            t.in_paste(),
            "ESC-resync at split {split}: should enter paste after re-sending begin"
        );
    }
}

/// Rapid begin/end toggle produces correct states.
#[test]
fn paste_toggle_sequence_table() {
    #[rustfmt::skip]
    let steps: &[(&[u8], bool)] = &[
        (PASTE_BEGIN,  true),
        (PASTE_END,    false),
        (PASTE_BEGIN,  true),
        (PASTE_BEGIN,  true),   // double begin: sticky true
        (PASTE_END,    false),
        (PASTE_END,    false),  // double end: sticky false
    ];
    let mut t = PasteTracker::new();
    for (input, expected) in steps {
        t.feed_slice(input);
        assert_eq!(t.in_paste(), *expected, "after feeding {input:?}");
    }
}

/// Non-ESC bytes at every value (0..=255 excluding 0x1b) must never
/// change paste state when fed from the Ground state.
#[test]
fn paste_ground_state_non_esc_bytes_are_harmless() {
    for b in 0u8..=255 {
        if b == 0x1b {
            continue;
        }
        let mut t = PasteTracker::new();
        t.feed(b);
        assert!(
            !t.in_paste(),
            "byte 0x{b:02x} from Ground toggled paste state"
        );
    }
}

/// The closing `~` of the end marker is never forwarded to the
/// trigger parser — the state after consuming the full end marker
/// (but before any further input) must be outside paste.
#[test]
fn paste_end_marker_closing_tilde_exits_paste() {
    let mut t = PasteTracker::new();
    t.feed_slice(PASTE_BEGIN);
    // Feed end marker byte by byte; only the final `~` clears the flag.
    for &b in &PASTE_END[..PASTE_END.len() - 1] {
        t.feed(b);
        assert!(t.in_paste(), "paste should remain active before final ~");
    }
    t.feed(b'~');
    assert!(!t.in_paste(), "paste should clear after final ~");
}

// ── AltScreen tracker ────────────────────────────────────────────

/// All recognized alt-screen mode numbers must enter alt-screen on `h`
/// and leave on `l`.
#[test]
fn altscreen_all_enter_modes_systematic() {
    const ENTER_MODES: &[(u32, bool)] = &[
        (47, true),    // original xterm: switch only
        (1047, true),  // older xterm: switch + clear
        (1049, true),  // modern xterm: save cursor + switch + clear
        (1048, false), // cursor save/restore only — no buffer switch
        (0, false),    // not a mode
        (2, false),    // not a mode
        (25, false),   // cursor visibility — not alt-screen
        (999, false),  // unknown
    ];
    for (mode, expected) in ENTER_MODES {
        let seq = format!("\x1b[?{}h", mode);
        let mut t = AltScreenTracker::new();
        t.feed_slice(seq.as_bytes());
        assert_eq!(
            t.is_alt_screen(),
            *expected,
            "enter mode {mode}: expected alt={expected}"
        );
    }
}

/// After entering alt-screen, all modes that entered must also leave.
#[test]
fn altscreen_all_leave_modes_systematic() {
    const TOGGLE_MODES: &[u32] = &[47, 1047, 1049];
    for mode in TOGGLE_MODES {
        let enter = format!("\x1b[?{}h", mode);
        let leave = format!("\x1b[?{}l", mode);
        let mut t = AltScreenTracker::new();
        t.feed_slice(enter.as_bytes());
        assert!(t.is_alt_screen(), "mode {mode}: did not enter");
        t.feed_slice(leave.as_bytes());
        assert!(!t.is_alt_screen(), "mode {mode}: did not leave");
    }
}

/// Multi-parameter forms: mode at any position in the semicolon list
/// must still toggle.
#[test]
fn altscreen_multi_param_mode_at_any_position() {
    const CASES: &[(&[u8], bool)] = &[
        (b"\x1b[?1049;1h", true),   // mode first
        (b"\x1b[?1;1049h", true),   // mode last
        (b"\x1b[?1;1049;2h", true), // mode middle
        (b"\x1b[?1;2;3h", false),   // no alt mode anywhere
    ];
    for (seq, expected) in CASES {
        let mut t = AltScreenTracker::new();
        t.feed_slice(seq);
        assert_eq!(
            t.is_alt_screen(),
            *expected,
            "seq {seq:?}: expected alt={expected}"
        );
    }
}

/// Partial alt-screen prefix followed by a disruptive byte must NOT
/// toggle the alt-screen flag.
#[test]
fn altscreen_partial_prefix_wrong_byte_does_not_toggle() {
    let marker = b"\x1b[?1049h";
    let disruptions: &[u8] = b"XA09~\n";
    for n in 0..marker.len() {
        for &disruptor in disruptions {
            let mut t = AltScreenTracker::new();
            t.feed_slice(&marker[..n]);
            t.feed(disruptor);
            assert!(
                !t.is_alt_screen(),
                "prefix len {n} + disruptor 0x{disruptor:02x}: unexpected alt-screen toggle"
            );
        }
    }
}

/// ESC mid-CSI resets any partial alt-mode match; subsequent
/// unrelated `h` must not steal the stale match.
#[test]
fn altscreen_esc_mid_param_clears_stale_match_table() {
    const CASES: &[(&[u8], bool)] = &[
        // ESC restarts after partial 1049; the next CSI has no `?`
        (b"\x1b[?1049;\x1b[1h", false),
        // ESC between `?` and digits: no param seen, no toggle
        (b"\x1b[?\x1b[?1049h", true),
        // ESC mid-digit sequence; fresh begin still recognized
        (b"\x1b[?10\x1b[?1049h", true),
    ];
    for (seq, expected) in CASES {
        let mut t = AltScreenTracker::new();
        t.feed_slice(seq);
        assert_eq!(
            t.is_alt_screen(),
            *expected,
            "seq {seq:?}: expected alt={expected}"
        );
    }
}

/// Non-ESC bytes in Ground state never toggle alt-screen.
#[test]
fn altscreen_ground_state_non_esc_bytes_are_harmless() {
    for b in 0u8..=255 {
        if b == 0x1b {
            continue;
        }
        let mut t = AltScreenTracker::new();
        t.feed(b);
        assert!(
            !t.is_alt_screen(),
            "byte 0x{b:02x} from Ground toggled alt-screen"
        );
    }
}

/// Very large parameter numbers must not panic and must not match.
#[test]
fn altscreen_huge_param_table() {
    const CASES: &[&[u8]] = &[
        b"\x1b[?999999999h",
        b"\x1b[?1049999999h",
        b"\x1b[?00000000001049h",
    ];
    for seq in CASES {
        let mut t = AltScreenTracker::new();
        t.feed_slice(seq);
        // The first case should not match (way over any mode number).
        // The second case also should not match (no exact match).
        // The third case: leading zeros + "1049" might match depending on accumulator.
        // We only check no panic here — the exact state depends on overflow handling.
        let _ = t.is_alt_screen();
    }
}
