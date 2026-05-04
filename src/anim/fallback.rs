//! Plain-text fallback for narrow terminals.
//!
//! When the terminal is below 40 columns ([`crate::term::width::
//! WidthBucket::Tiny`]) the ambulance ASCII car cannot fit, so
//! the renderer drops the art entirely and emits a single-line
//! text gag instead. This module owns both the gag strings and
//! the trigger → gag mapping; the upcoming crossterm renderer
//! (#47) picks between [`fallback`] and [`super::frame::frame`]
//! based on the active [`crate::term::width::WidthBucket`].
//!
//! The gag strings are intentionally kept short enough to fit
//! in a single line when the terminal is at least 33 columns wide
//! (the widest gag is 33 chars). Terminals narrower than 33
//! columns receive a best-effort output that may wrap.
//!
//! Pure ASCII, single line, no trailing terminator: callers
//! append their own line-end so this layer stays stylistic-only.

/// A fired quit trigger that the input pump observed.
///
/// Corresponds to [`crate::trigger::parser::Outcome`] minus the
/// `None` variant: the fallback gag is only meaningful once a
/// trigger has actually fired, so we model "no gag" with the
/// absence of a `Trigger` value rather than a fourth variant. The
/// renderer is responsible for translating an
/// [`crate::trigger::parser::Outcome`] into an `Option<Trigger>`
/// before calling [`fallback`].
///
/// Note: variant names follow `Outcome` except that
/// `Outcome::QBang` maps to [`Trigger::Bang`] to avoid the
/// redundant `Q` prefix inside this enum's namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// `:q` -- standard ambulance gag.
    Q,
    /// `:wq` -- ambulance carrying the spec-locked 418 label.
    Wq,
    /// `:q!` — nine-car parade gag.
    ///
    /// Maps from [`crate::trigger::parser::Outcome::QBang`]; the
    /// shorter name avoids redundancy with the enum prefix.
    Bang,
}

/// Single-line plain-text gag emitted at `cols < 40`.
///
/// The returned string is `'static`, pure ASCII, and contains no
/// `\n`. Width is held under 34 columns so the gag fits in a
/// 33-column terminal; this is verified by
/// `gags_fit_under_thirty_four_columns` below.
///
/// The `[QQ]` prefix mirrors the compact ambulance header used
/// by the `Small` usage screen (see [`crate::usage::screen`]) so
/// the fallback feels like the same character muted by the
/// terminal width budget rather than an unrelated message.
pub fn fallback(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Q => "[QQ] Fi-Fo-Fi-Fo... :q copy that.",
        Trigger::Wq => "[QQ] :wq -- 418 I'm an AI agent.",
        Trigger::Bang => "[QQ]x9 :q! parade -- 9! = 362880.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exhaustive list of the three trigger variants. Adding a
    /// new variant must update this constant *and* the `match` in
    /// [`fallback`]; the compiler's exhaustiveness check
    /// guarantees the second half but not the first, so this
    /// table guards the test surface.
    const ALL_TRIGGERS: [Trigger; 3] = [Trigger::Q, Trigger::Wq, Trigger::Bang];

    /// Each trigger maps to a non-empty gag.
    #[test]
    fn every_trigger_has_a_gag() {
        for t in ALL_TRIGGERS {
            let gag = fallback(t);
            assert!(!gag.is_empty(), "{t:?} returned empty gag");
        }
    }

    /// Distinct triggers produce distinct gags so users can tell
    /// which command fired even on the narrow fallback path.
    #[test]
    fn gags_are_pairwise_distinct() {
        for (i, a) in ALL_TRIGGERS.iter().enumerate() {
            for b in ALL_TRIGGERS.iter().skip(i + 1) {
                assert_ne!(fallback(*a), fallback(*b), "{a:?} and {b:?} share a gag",);
            }
        }
    }

    /// Fallback output is single-line: no embedded `\n` (callers
    /// add their own terminator) and no embedded `\r`.
    #[test]
    fn gags_are_single_line() {
        for t in ALL_TRIGGERS {
            let gag = fallback(t);
            assert!(!gag.contains('\n'), "{t:?} contained LF: {gag:?}");
            assert!(!gag.contains('\r'), "{t:?} contained CR: {gag:?}");
        }
    }

    /// Pure ASCII keeps `chars().count()` equivalent to printed
    /// columns, matching the rest of the anim layer.
    #[test]
    fn gags_are_pure_ascii() {
        for t in ALL_TRIGGERS {
            let gag = fallback(t);
            assert!(gag.is_ascii(), "{t:?} contained non-ASCII: {gag:?}");
        }
    }

    /// All gags fit in a 33-column terminal so terminals in the
    /// 33–39 column range of the fallback bucket render them
    /// without wrapping. The hard cap equals the longest current
    /// gag (33 cols); any addition that makes a gag wider fails
    /// this test immediately.
    #[test]
    fn gags_fit_under_thirty_four_columns() {
        const LIMIT: usize = 33;
        for t in ALL_TRIGGERS {
            let gag = fallback(t);
            let cols = gag.chars().count();
            assert!(
                cols <= LIMIT,
                "{t:?} gag is {cols} cols, exceeds {LIMIT}: {gag:?}",
            );
        }
    }

    /// Each gag references the triggering literal so the user can
    /// confirm what the wrapper observed.
    #[test]
    fn gags_reference_their_trigger_literal() {
        assert!(fallback(Trigger::Q).contains(":q"));
        assert!(fallback(Trigger::Wq).contains(":wq"));
        assert!(fallback(Trigger::Bang).contains(":q!"));
    }

    /// The `:wq` gag must carry the spec-locked 418 label
    /// verbatim (issue #11 §4, asset `big.txt`).
    #[test]
    fn wq_gag_carries_spec_locked_418_label() {
        let gag = fallback(Trigger::Wq);
        assert!(
            gag.contains("418 I'm an AI agent"),
            "wq gag missing spec-locked label: {gag:?}",
        );
    }

    /// The `:q!` gag references the factorial joke from the
    /// usage screen (`9! = 362880`) so the parade scene's
    /// punchline survives the narrow-terminal degradation.
    #[test]
    fn bang_gag_keeps_factorial_punchline() {
        let gag = fallback(Trigger::Bang);
        assert!(
            gag.contains("362880"),
            "bang gag missing 9! = 362880: {gag:?}",
        );
    }

    /// `Trigger` is a small `Copy` enum; lock that so callers can
    /// pass it without `clone()` and pattern-match it freely.
    #[test]
    fn trigger_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Trigger>();
    }
}
