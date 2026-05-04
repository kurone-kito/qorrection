//! Pure frame generator for the moving-car animation.
//!
//! Composes a single rendered frame from three inputs: a car
//! ASCII asset (one of the variants in [`crate::anim::car`]), the
//! car's horizontal position, and the current siren phase. The
//! output is a multi-line `String` that joins rendered rows with
//! `\n`. When the last rendered row is non-empty the string has
//! no trailing newline; when the last row is empty after clipping
//! the preceding `\n` separator becomes the final byte.
//!
//! The function is intentionally oblivious to terminal width,
//! scene type, and animation timing: scene orchestrators (Phase
//! F #43-#45) walk this generator across an `x` range and a
//! siren cycle to drive the visible motion. Right-side clipping
//! is also the caller's job — this module only handles the
//! left-edge geometry (positive offsets pad with spaces, negative
//! offsets clip into the body).
//!
//! Pure ASCII in, pure ASCII out: the asset bytes stay 7-bit and
//! the siren trail is built from `'F'`, `'i'`, `'o'`, `'-'` only,
//! so `chars().count()` equals the printed column count.

use crate::anim::car;

/// Two-state FI-FO siren cycle.
///
/// Scenes flip the phase every animation tick so the trailing
/// label oscillates `Fi -> Fo -> Fi -> ...`, producing the
/// audible-feeling "FI-FO-FI-FO" trail from the spec lock (issue
/// #11 §4). The phase encodes which syllable currently *leads*
/// the trail; the second syllable always follows immediately
/// after a separator dash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SirenPhase {
    /// Trail leads with `Fi`.
    #[default]
    Fi,
    /// Trail leads with `Fo`.
    Fo,
}

impl SirenPhase {
    /// The two-character syllable currently leading the trail.
    pub fn syllable(self) -> &'static str {
        match self {
            Self::Fi => "Fi",
            Self::Fo => "Fo",
        }
    }

    /// Toggle to the opposite phase. Two flips return to start.
    pub fn flip(self) -> Self {
        match self {
            Self::Fi => Self::Fo,
            Self::Fo => Self::Fi,
        }
    }
}

/// Render a single animation frame.
///
/// `car_asset` is the multi-line ASCII art (typically
/// [`car::TINY`], [`car::STD`], or [`car::BIG`]) and must be
/// pure ASCII (`debug_assert!`-enforced in debug builds); the
/// function does not otherwise validate the asset's identity.
///
/// `x_offset` is the column of the car's leftmost edge:
///
/// - `x_offset == 0`: every car row starts at column 0; no siren
///   trail is drawn (there is no canvas to its left).
/// - `x_offset > 0`: each non-wheel row is padded with `x_offset`
///   leading spaces; the wheel row (the last row of the asset)
///   is prefixed with the siren trail filling those columns.
/// - `x_offset < 0`: the leading `(-x_offset)` characters of
///   every row are clipped (car partially off-screen on the
///   left). When `(-x_offset)` exceeds a row's width the row is
///   emitted as an empty line. No siren trail is drawn — the
///   trail only exists when there is canvas to the *left* of the
///   car.
///
/// `phase` selects which syllable leads the siren trail when one
/// is drawn. See [`SirenPhase`].
///
/// The returned string joins rendered rows with `\n`. When the
/// final rendered row is non-empty the string has no trailing
/// newline; when it is empty (e.g. a large negative `x_offset`
/// clips the entire last row) the preceding `\n` separator is the
/// last byte. The row count always equals
/// `car::lines(car_asset).len()`.
pub fn frame(car_asset: &str, x_offset: i32, phase: SirenPhase) -> String {
    debug_assert!(car_asset.is_ascii(), "frame() requires ASCII car_asset");
    let lines = car::lines(car_asset);
    let last_idx = lines.len().saturating_sub(1);
    let trail_width = x_offset.max(0) as usize;
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if x_offset >= 0 {
            if i == last_idx {
                push_siren_trail(&mut out, phase, trail_width);
            } else {
                for _ in 0..trail_width {
                    out.push(' ');
                }
            }
            out.push_str(line);
        } else {
            let drop = x_offset.unsigned_abs() as usize;
            if let Some(tail) = line.get(drop..) {
                out.push_str(tail);
            }
        }
        if i < last_idx {
            out.push('\n');
        }
    }
    out
}

/// Push exactly `width` characters of the siren trail into `out`.
///
/// The repeating unit is `"Fi-Fo-"` (or `"Fo-Fi-"` when `phase`
/// is [`SirenPhase::Fo`]) — six characters covering both
/// syllables and their separators. The function repeats the unit
/// and truncates to `width`, so very small widths still emit a
/// useful prefix (e.g. `width == 1` → `"F"`, `width == 5` →
/// `"Fi-Fo"`).
fn push_siren_trail(out: &mut String, phase: SirenPhase, width: usize) {
    if width == 0 {
        return;
    }
    let unit: &str = match phase {
        SirenPhase::Fi => "Fi-Fo-",
        SirenPhase::Fo => "Fo-Fi-",
    };
    let unit_bytes = unit.as_bytes();
    out.reserve(width);
    for i in 0..width {
        out.push(unit_bytes[i % unit_bytes.len()] as char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `x == 0` reproduces the asset verbatim (with no trailing
    /// newline) and never emits a siren trail.
    #[test]
    fn frame_at_zero_emits_car_only() {
        let out = frame(car::TINY, 0, SirenPhase::Fi);
        let expected = car::lines(car::TINY).join("\n");
        assert_eq!(out, expected);
        assert!(!out.contains("Fi"), "no trail at x=0; got {out:?}");
    }

    /// Positive `x` pads non-wheel rows with leading spaces; only
    /// the wheel row carries the siren trail.
    #[test]
    fn frame_pads_top_rows_with_spaces() {
        let out = frame(car::TINY, 6, SirenPhase::Fi);
        let lines: Vec<&str> = out.split('\n').collect();
        let car_lines = car::lines(car::TINY);
        assert_eq!(lines.len(), car_lines.len());
        for (i, l) in lines.iter().enumerate().take(lines.len() - 1) {
            assert!(
                l.starts_with("      "),
                "non-wheel row {i} missing space pad; got {l:?}",
            );
            assert!(
                !l.contains("Fi") && !l.contains("Fo"),
                "non-wheel row {i} carries trail; got {l:?}",
            );
        }
        let wheel = lines.last().unwrap();
        assert!(
            wheel.starts_with("Fi-Fo-"),
            "wheel row missing trail; got {wheel:?}",
        );
    }

    /// At small positive `x` the trail is the head of one repeating
    /// unit, including degenerate cases like `width == 1`.
    #[test]
    fn frame_short_trail_truncates_unit() {
        for (x, expected) in [
            (1, "F"),
            (2, "Fi"),
            (3, "Fi-"),
            (4, "Fi-F"),
            (5, "Fi-Fo"),
            (6, "Fi-Fo-"),
            (7, "Fi-Fo-F"),
        ] {
            let out = frame(car::TINY, x, SirenPhase::Fi);
            let wheel = out.rsplit('\n').next().unwrap();
            assert!(
                wheel.starts_with(expected),
                "x={x}: wheel {wheel:?} does not start with {expected:?}",
            );
        }
    }

    /// Phase `Fo` swaps the leading syllable.
    #[test]
    fn frame_phase_fo_swaps_siren_lead() {
        let out = frame(car::TINY, 12, SirenPhase::Fo);
        let wheel = out.rsplit('\n').next().unwrap();
        assert!(
            wheel.starts_with("Fo-Fi-Fo-Fi-"),
            "wheel {wheel:?} does not lead with Fo",
        );
    }

    /// Negative offsets clip the leading characters from every
    /// row and never emit a trail.
    #[test]
    fn frame_negative_offset_clips_left() {
        let out = frame(car::TINY, -3, SirenPhase::Fi);
        let lines: Vec<&str> = out.split('\n').collect();
        let car_lines = car::lines(car::TINY);
        assert_eq!(lines.len(), car_lines.len());
        for (i, (l, c)) in lines.iter().zip(car_lines.iter()).enumerate() {
            let expected = if c.len() > 3 { &c[3..] } else { "" };
            assert_eq!(*l, expected, "row {i} clipped wrong");
        }
        assert!(!out.contains("Fi") && !out.contains("Fo"));
    }

    /// `x_offset == i32::MIN` must not overflow (negating i32::MIN
    /// panics in debug and wraps in release). All rows are clipped
    /// to empty, and the row count is preserved.
    #[test]
    fn frame_i32_min_does_not_panic() {
        let out = frame(car::TINY, i32::MIN, SirenPhase::Fi);
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines.len(), car::lines(car::TINY).len());
        for l in lines {
            assert!(l.is_empty(), "row not empty: {l:?}");
        }
    }

    /// A clip wider than the longest car row produces all-empty
    /// rows, but the row count is still preserved so animation
    /// timelines do not fall out of sync.
    #[test]
    fn frame_negative_offset_beyond_line_length_emits_empty() {
        let out = frame(car::TINY, -100, SirenPhase::Fi);
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines.len(), car::lines(car::TINY).len());
        for l in lines {
            assert!(l.is_empty(), "row not empty: {l:?}");
        }
    }

    /// Non-degenerate frames (those with visible content on the
    /// last row) end without a trailing newline so callers can
    /// append cursor or clear sequences without an extra blank
    /// row. When the entire frame is clipped to empty rows the
    /// output is exactly `(n_rows - 1)` newlines, which is the
    /// natural `lines.join("\n")` semantics; the row-count
    /// contract still holds.
    #[test]
    fn frame_has_no_trailing_newline_when_last_row_visible() {
        // TINY's wheel row is 8 chars; x = -7 still leaves one
        // visible character on it, so no trailing newline.
        for x in [-7i32, -3, 0, 1, 5, 12, 200] {
            for phase in [SirenPhase::Fi, SirenPhase::Fo] {
                let out = frame(car::TINY, x, phase);
                assert!(
                    !out.ends_with('\n'),
                    "x={x} phase={phase:?}: trailing newline in {out:?}",
                );
            }
        }
    }

    /// When the last row is fully clipped the preceding `\n`
    /// separator becomes the last byte — the "no trailing newline"
    /// guarantee only applies when the final row is non-empty.
    ///
    /// Uses a synthetic short-last-row asset so the clip is
    /// unambiguous: x=-3 clips the 2-char last row completely
    /// while leaving content on the first two rows.
    #[test]
    fn frame_last_row_clipped_ends_with_newline() {
        // 3-row asset where the last row (2 chars) is shorter than
        // the first two (5 chars each).  car::lines strips the
        // trailing \n before splitting.
        let asset = "AAAAA\nBBBBB\nCC\n";
        let out = frame(asset, -3, SirenPhase::Fi);
        // x=-3 clips 3 chars: rows 0 and 1 ("AAAAA","BBBBB") each
        // yield 2 visible chars; row 2 ("CC") is fully clipped.
        // Expected: "AA\nBB\n" (the last \n is the row-1 separator,
        // not a new terminator).
        assert_eq!(
            out, "AA\nBB\n",
            "expected visible rows + trailing separator; got {out:?}",
        );
    }

    /// Output is always pure ASCII (the assets are ASCII and the
    /// trail uses only `F i o -`), so width math stays honest.
    #[test]
    fn frame_is_pure_ascii() {
        for asset in [car::TINY, car::STD, car::BIG] {
            for x in [-5i32, 0, 7, 40] {
                for phase in [SirenPhase::Fi, SirenPhase::Fo] {
                    let out = frame(asset, x, phase);
                    assert!(out.is_ascii(), "non-ASCII bytes for x={x} phase={phase:?}",);
                }
            }
        }
    }

    /// Row count is always the asset's row count, regardless of
    /// `x` or phase. Scenes rely on this to align frames with a
    /// fixed cursor-up count.
    #[test]
    fn frame_row_count_matches_asset() {
        for asset in [car::TINY, car::STD, car::BIG] {
            let expected = car::lines(asset).len();
            for x in [-100i32, -1, 0, 1, 50] {
                let out = frame(asset, x, SirenPhase::Fi);
                let got = out.split('\n').count();
                assert_eq!(got, expected, "asset rows={expected} x={x} got={got}");
            }
        }
    }

    /// The empty asset edge-case: `lines.len()` is 1 (an empty
    /// string splits into one empty line). `frame` must not panic;
    /// with `x=5` the wheel row is empty so the output is the
    /// 5-character siren trail `"Fi-Fo"` with no car body.
    #[test]
    fn frame_empty_asset_does_not_panic() {
        let out = frame("", 5, SirenPhase::Fi);
        assert_eq!(out, "Fi-Fo");
    }

    #[test]
    fn siren_phase_flip_is_involutive() {
        assert_eq!(SirenPhase::Fi.flip(), SirenPhase::Fo);
        assert_eq!(SirenPhase::Fo.flip(), SirenPhase::Fi);
        assert_eq!(SirenPhase::Fi.flip().flip(), SirenPhase::Fi);
    }

    #[test]
    fn siren_phase_syllables_are_two_ascii_chars() {
        for phase in [SirenPhase::Fi, SirenPhase::Fo] {
            let s = phase.syllable();
            assert_eq!(s.len(), 2);
            assert!(s.is_ascii());
        }
        assert_ne!(
            SirenPhase::Fi.syllable(),
            SirenPhase::Fo.syllable(),
            "syllables must differ",
        );
    }

    #[test]
    fn siren_phase_default_is_fi() {
        assert_eq!(SirenPhase::default(), SirenPhase::Fi);
    }
}
