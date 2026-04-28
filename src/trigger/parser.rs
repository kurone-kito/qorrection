//! Literal `:q` / `:wq` / `:q!` parser.
//!
//! Sees the **raw byte stream** the user typed (stdin → child
//! PTY). The Phase E input pump asks every byte three questions
//! in order: paste tracker → alt-screen tracker → parser. When
//! the first two say "disarmed" the parser is bypassed entirely;
//! the bytes still flow to the child PTY unchanged.
//!
//! Grammar (lines are terminated by CR, LF, or CRLF):
//!
//! ```text
//!     line      ::= leading_ws* literal? rest? terminator
//!     leading_ws::= ' ' | '\t'
//!     literal   ::= ":q" | ":wq" | ":q!"
//!     rest      ::= any printable bytes that turn `literal` into
//!                   a non-match (treated as trailing garbage)
//!     terminator::= '\r' | '\n' | '\r\n'
//! ```
//!
//! A line matches a trigger only when, after stripping leading
//! whitespace, the body is exactly one of the three literals and
//! the line was never tainted by any control byte (backspace,
//! Ctrl-U, embedded tab, embedded ESC, etc.). The taint rule is
//! intentionally strict: if the user reached for editing keys
//! mid-line, we cannot reliably know what they meant to type, so
//! we refuse to match. This costs nothing — the user can always
//! retype the literal — and prevents false positives against
//! `:qX\bq\n` style adversarial inputs.
//!
//! Cross-read-boundary safety: because `feed` is byte-at-a-time
//! and the only state-affecting events are line terminators, an
//! arbitrary split of any input stream produces the same outcome
//! sequence as feeding the concatenated stream in one go.
//!
//! ## Phase E pump contract
//!
//! This module classifies; the Phase E input pump owns which
//! bytes reach the child PTY. When `feed` returns a non-`None`
//! outcome on `\r`, the immediately following `\n` (if any) is
//! the second half of a CRLF terminator and must be suppressed
//! by the pump as well. The pump must also call [`Parser::reset`]
//! on every armed/disarmed boundary (entering or leaving paste
//! mode, alt-screen mode, or any other window during which bytes
//! are routed past this parser) so that a partial dirty line
//! cannot survive a bypass and poison the next clean line.

/// What the parser saw at end-of-line.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum Outcome {
    /// Line did not match any trigger literal.
    #[default]
    None,
    /// `:q` — standard ambulance, FI-FO-FI-FO siren.
    Q,
    /// `:wq` — bigger ambulance carrying the 418 label.
    Wq,
    /// `:q!` — nine-car parade.
    QBang,
}

/// Soft cap on buffered body bytes per line. The longest matchable
/// literal is `:wq` (3 bytes); anything past 64 bytes cannot match
/// any trigger and is just trailing garbage we'd discard anyway.
/// Capping prevents pathological streams from growing the buffer
/// without bound.
const MAX_BODY: usize = 64;

#[derive(Debug, Default)]
pub struct Parser {
    /// Body bytes seen on the current line (after leading WS).
    /// Cleared on every terminator. Capacity is bounded by
    /// [`MAX_BODY`].
    buf: Vec<u8>,
    /// Sticky for the current line: once true, the line cannot
    /// match. Cleared on terminator. Set when we see a control
    /// byte mid-body, an ESC anywhere, or when the body grows
    /// past [`MAX_BODY`].
    dirty: bool,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume one byte; return the trigger outcome at line end.
    /// Non-terminator bytes always return [`Outcome::None`].
    pub fn feed(&mut self, b: u8) -> Outcome {
        match b {
            b'\r' | b'\n' => {
                let outcome = self.classify();
                self.reset_line();
                outcome
            }
            // Leading whitespace: silently skipped while the body
            // is empty. Once any body byte arrives, ' '/'\t' would
            // fall through to the "printable" branch (' ') or the
            // "control byte" branch ('\t' is 0x09), which is the
            // correct behaviour for the trailing-garbage and
            // embedded-tab cases respectively.
            b' ' | b'\t' if self.buf.is_empty() && !self.dirty => Outcome::None,
            // Any C0 control byte (other than the terminators we
            // already matched) or DEL taints the line. This
            // covers backspace (0x08), tab in body (0x09), Ctrl-U
            // (0x15), ESC (0x1B), and friends.
            0x00..=0x1f | 0x7f => {
                self.dirty = true;
                Outcome::None
            }
            _ => {
                if self.buf.len() >= MAX_BODY {
                    // Past the cap, no future state could match.
                    self.dirty = true;
                } else {
                    self.buf.push(b);
                }
                Outcome::None
            }
        }
    }

    fn classify(&self) -> Outcome {
        if self.dirty {
            return Outcome::None;
        }
        match self.buf.as_slice() {
            b":q" => Outcome::Q,
            b":wq" => Outcome::Wq,
            b":q!" => Outcome::QBang,
            _ => Outcome::None,
        }
    }

    fn reset_line(&mut self) {
        self.buf.clear();
        self.dirty = false;
    }

    /// Public disarm-boundary reset for the Phase E pump. Call
    /// this whenever the byte stream is about to be routed past
    /// the parser (e.g. entering bracketed paste or alt-screen)
    /// and again when the parser is rearmed, so a partial dirty
    /// line cannot survive across a bypass window.
    pub fn reset(&mut self) {
        self.reset_line();
    }

    /// Test/diagnostic helper: feed a slice and return the
    /// sequence of non-`None` outcomes in order.
    pub fn feed_all(&mut self, bytes: &[u8]) -> Vec<Outcome> {
        let mut out = Vec::new();
        for &b in bytes {
            let o = self.feed(b);
            if o != Outcome::None {
                out.push(o);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(parser: &mut Parser, bytes: &[u8]) -> Outcome {
        parser
            .feed_all(bytes)
            .into_iter()
            .next()
            .unwrap_or(Outcome::None)
    }

    // ---------- positive cases ----------

    #[test]
    fn bare_q_matches_with_lf() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":q\n"), Outcome::Q);
    }

    #[test]
    fn bare_wq_matches_with_lf() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":wq\n"), Outcome::Wq);
    }

    #[test]
    fn bare_q_bang_matches_with_lf() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":q!\n"), Outcome::QBang);
    }

    #[test]
    fn cr_only_terminator_matches() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":q\r"), Outcome::Q);
    }

    #[test]
    fn crlf_terminator_matches_once() {
        let mut p = Parser::new();
        // CR fires the match; LF runs against an empty buffer
        // and produces no extra outcome.
        assert_eq!(p.feed_all(b":wq\r\n"), vec![Outcome::Wq]);
    }

    #[test]
    fn leading_spaces_allowed() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b"   :q\n"), Outcome::Q);
    }

    #[test]
    fn leading_tabs_allowed() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b"\t\t:q!\n"), Outcome::QBang);
    }

    #[test]
    fn mixed_leading_whitespace_allowed() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b" \t \t:wq\n"), Outcome::Wq);
    }

    // ---------- negative cases ----------

    #[test]
    fn trailing_garbage_after_q_rejected() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":q foo\n"), Outcome::None);
    }

    #[test]
    fn trailing_text_directly_after_literal_rejected() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":qq\n"), Outcome::None);
        assert_eq!(first(&mut p, b":wqx\n"), Outcome::None);
        assert_eq!(first(&mut p, b":q!!\n"), Outcome::None);
    }

    #[test]
    fn embedded_backspace_rejected_even_if_visually_yields_q() {
        // User types :, q, X, BS, q, ENTER. A real terminal might
        // render `:qq` after the BS, but our raw-byte parser sees
        // a control byte mid-line and refuses.
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":qX\x08q\n"), Outcome::None);
    }

    #[test]
    fn embedded_del_127_rejected() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":qX\x7fq\n"), Outcome::None);
    }

    #[test]
    fn ctrl_u_mid_line_rejected_even_if_visually_yields_q() {
        // Ctrl-U (0x15) clears the line in many shells, so the
        // visible result of `:qjunk^U:q\n` would be `:q`. We still
        // refuse because the raw stream contains a control byte.
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":qjunk\x15:q\n"), Outcome::None);
    }

    #[test]
    fn embedded_esc_taints_line() {
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":\x1b[Aq\n"), Outcome::None);
    }

    #[test]
    fn embedded_tab_after_body_rejected() {
        // Leading tab is fine, but a tab after a body byte taints.
        let mut p = Parser::new();
        assert_eq!(first(&mut p, b":q\t\n"), Outcome::None);
        assert_eq!(first(&mut p, b":\tq\n"), Outcome::None);
    }

    #[test]
    fn empty_line_no_match() {
        let mut p = Parser::new();
        assert_eq!(p.feed_all(b"\n\r\n\r"), vec![]);
    }

    #[test]
    fn case_sensitive_literals() {
        let mut p = Parser::new();
        for s in [
            b":Q\n".as_ref(),
            b":WQ\n".as_ref(),
            b":Q!\n".as_ref(),
            b"  :Q\n".as_ref(),
        ] {
            assert_eq!(first(&mut p, s), Outcome::None, "{s:?}");
        }
    }

    #[test]
    fn very_long_line_does_not_grow_unbounded() {
        let mut p = Parser::new();
        // Feed many bytes worth of garbage on a single line. The
        // buffer must stay bounded by MAX_BODY, and the line must
        // be marked dirty so it cannot match.
        let huge: Vec<u8> = std::iter::repeat(b'a').take(10_000).collect();
        for &b in &huge {
            assert_eq!(p.feed(b), Outcome::None);
        }
        assert!(p.buf.len() <= MAX_BODY);
        assert!(p.dirty);
        assert_eq!(p.feed(b'\n'), Outcome::None);
        // The terminator must clear dirty so the next line is
        // not poisoned by the cap.
        assert_eq!(p.feed_all(b":q\n"), vec![Outcome::Q]);
    }

    #[test]
    fn leading_control_bytes_taint_the_line() {
        for ctrl in [0x08u8, 0x15, 0x1b] {
            let mut p = Parser::new();
            let stream = [ctrl, b':', b'q', b'\n'];
            assert_eq!(
                p.feed_all(&stream),
                vec![],
                "leading 0x{ctrl:02x} should taint"
            );
            // Next clean line must still match.
            assert_eq!(p.feed_all(b":q\n"), vec![Outcome::Q]);
        }
    }

    #[test]
    fn all_literals_accept_cr_lf_and_crlf() {
        // Lock the full 3-literal x 3-terminator grid so future
        // refactors cannot regress one cell silently.
        let cases: &[(&[u8], Outcome)] = &[
            (b":q\r", Outcome::Q),
            (b":q\n", Outcome::Q),
            (b":q\r\n", Outcome::Q),
            (b":wq\r", Outcome::Wq),
            (b":wq\n", Outcome::Wq),
            (b":wq\r\n", Outcome::Wq),
            (b":q!\r", Outcome::QBang),
            (b":q!\n", Outcome::QBang),
            (b":q!\r\n", Outcome::QBang),
        ];
        for (stream, expected) in cases {
            let mut p = Parser::new();
            assert_eq!(p.feed_all(stream), vec![*expected], "stream={stream:?}");
        }
    }

    #[test]
    fn public_reset_clears_partial_dirty_line() {
        // Models the Phase E disarm boundary: parser collects a
        // partial tainted line, then the pump diverts bytes
        // (paste / alt-screen window). On rearm it calls reset();
        // the next clean trigger must fire.
        let mut p = Parser::new();
        p.feed(b':');
        p.feed(b'q');
        p.feed(b'X');
        p.feed(0x08); // backspace -> dirty
        p.reset();
        assert_eq!(p.feed_all(b":q\n"), vec![Outcome::Q]);
    }

    #[test]
    fn negative_lines_split_at_every_byte_yield_no_match() {
        for stream in [
            b":q foo\n".as_ref(),
            b":qX\x08q\n".as_ref(),
            b":qjunk\x15:q\n".as_ref(),
            b":q\t\n".as_ref(),
            b":\x1b[Aq\n".as_ref(),
        ] {
            for split in 0..=stream.len() {
                let mut p = Parser::new();
                let (a, b) = stream.split_at(split);
                let mut got = Vec::new();
                got.extend(p.feed_all(a));
                got.extend(p.feed_all(b));
                assert_eq!(got, vec![], "stream={stream:?} split={split}");
            }
        }
    }

    // ---------- cross-read-boundary safety ----------

    #[test]
    fn split_q_at_every_byte_position() {
        let line = b":q\n";
        for split in 0..=line.len() {
            let mut p = Parser::new();
            let (a, b) = line.split_at(split);
            let mut got = Vec::new();
            got.extend(p.feed_all(a));
            got.extend(p.feed_all(b));
            assert_eq!(got, vec![Outcome::Q], "split at {split}");
        }
    }

    #[test]
    fn split_wq_at_every_byte_position() {
        let line = b":wq\n";
        for split in 0..=line.len() {
            let mut p = Parser::new();
            let (a, b) = line.split_at(split);
            let mut got = Vec::new();
            got.extend(p.feed_all(a));
            got.extend(p.feed_all(b));
            assert_eq!(got, vec![Outcome::Wq], "split at {split}");
        }
    }

    #[test]
    fn split_q_bang_at_every_byte_position() {
        let line = b":q!\n";
        for split in 0..=line.len() {
            let mut p = Parser::new();
            let (a, b) = line.split_at(split);
            let mut got = Vec::new();
            got.extend(p.feed_all(a));
            got.extend(p.feed_all(b));
            assert_eq!(got, vec![Outcome::QBang], "split at {split}");
        }
    }

    #[test]
    fn split_crlf_terminator_does_not_double_fire() {
        let line = b":q\r\n";
        for split in 0..=line.len() {
            let mut p = Parser::new();
            let (a, b) = line.split_at(split);
            let mut got = Vec::new();
            got.extend(p.feed_all(a));
            got.extend(p.feed_all(b));
            assert_eq!(got, vec![Outcome::Q], "split at {split}");
        }
    }

    // ---------- multi-line streams ----------

    #[test]
    fn multiple_matches_in_a_stream() {
        let mut p = Parser::new();
        let out = p.feed_all(b":q\n:wq\n:q!\n");
        assert_eq!(out, vec![Outcome::Q, Outcome::Wq, Outcome::QBang]);
    }

    #[test]
    fn dirty_line_does_not_poison_next_line() {
        let mut p = Parser::new();
        let out = p.feed_all(b":qX\x08q\n:q\n");
        // First line tainted by BS; second line clean.
        assert_eq!(out, vec![Outcome::Q]);
    }
}
