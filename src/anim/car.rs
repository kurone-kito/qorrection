//! ASCII car assets used by the usage screen and the Phase F
//! animation scenes.
//!
//! The asset bytes are embedded with `include_str!` so the
//! shipped binary has no runtime file dependency. Each asset is
//! pure ASCII (no UTF-8 box-drawing, no emoji) so width
//! calculations using `str::chars().count()` are equivalent to
//! visible columns -- matters for the [`crate::usage::layout`]
//! two-column padding.
//!
//! The size taxonomy mirrors the spec's width buckets:
//!
//! | Variant        | Used at                     | Width budget |
//! | -------------- | --------------------------- | ------------ |
//! | [`TINY`]       | 40 ≤ cols < 80              | ≤ 12         |
//! | [`STD`]        | 80 ≤ cols < 120 (`:q`)      | ≤ 30         |
//! | [`BIG`]        | 120 ≤ cols < 160 (`:wq`)    | ≤ 45         |
//! | [`OVERSIZED`]  | cols ≥ 160 (`:wq` large)    | ≤ 100        |
//!
//! Below 40 cols the spec drops the ASCII car entirely and uses
//! a plain-text gag (Phase F `anim::fallback`); the assets here
//! simply do not apply.
//!
//! The [`OVERSIZED`] variant is reserved for the post-v0.1 oversized
//! cameo track defined in `docs/anim-large-art-contract.md`. It exposes
//! the asset so the future renderer (issue #118) can integrate it without
//! ad hoc path handling.

/// Compact 4-row car body for the 40-79 col bucket.
pub const TINY: &str = include_str!("assets/tiny.txt");
/// Standard 6-row car for the `:q` scene at ≥ 80 cols.
pub const STD: &str = include_str!("assets/std.txt");
/// Larger 7-row car for the `:wq` scene at 120–159 cols.
/// Body carries the two stacked labels per spec R5 / Q1:
/// `WRITE QUEUE` over `418 I'm an AI agent`.
pub const BIG: &str = include_str!("assets/big.txt");
/// Oversized 10-row locomotive body for the `:wq` large-terminal path
/// at ≥ 160 cols. Carries `418 I'M AN AI AGENT`, `WRITE QUEUE`, and
/// `:wq` per the oversized-scene contract (`docs/anim-large-art-contract.md`).
/// Reserved for post-v0.1 renderer integration (issue #118).
pub const OVERSIZED: &str = include_str!("assets/oversized.txt");

/// Split a raw asset into trimmed-of-trailing-newline lines.
///
/// `include_str!` always preserves the file's final `\n`, which
/// would otherwise show up as a phantom empty bottom row when
/// fed into the layout primitive. This helper is the single
/// place that policy lives.
pub fn lines(asset: &str) -> Vec<&str> {
    asset
        .strip_suffix('\n')
        .unwrap_or(asset)
        .split('\n')
        .collect()
}

/// Maximum visible width of any line in the asset, measured in
/// `chars().count()`. Pure-ASCII assets make this equivalent to
/// printed columns; the dimension test below pins that
/// invariant.
pub fn max_width(asset: &str) -> usize {
    lines(asset)
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
}

/// Number of visual rows after the trailing-newline strip.
pub fn height(asset: &str) -> usize {
    lines(asset).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each asset must hold its width budget so the layout
    /// primitive can right-pad with predictable spacing. Numbers
    /// taken from the spec lock §R8.
    #[test]
    fn assets_respect_width_budget() {
        assert!(
            max_width(TINY) <= 12,
            "tiny.txt exceeds 12 cols (was {})",
            max_width(TINY)
        );
        assert!(
            max_width(STD) <= 30,
            "std.txt exceeds 30 cols (was {})",
            max_width(STD)
        );
        assert!(
            max_width(BIG) <= 45,
            "big.txt exceeds 45 cols (was {})",
            max_width(BIG)
        );
        assert!(
            max_width(OVERSIZED) <= 100,
            "oversized.txt exceeds 100 cols (was {})",
            max_width(OVERSIZED)
        );
    }

    /// Heights reflect the table in the module doc; if any of
    /// these change, update the doc and the layout call sites.
    #[test]
    fn assets_have_expected_heights() {
        assert!(height(TINY) >= 3, "tiny too short: {}", height(TINY));
        assert!(height(STD) >= 5, "std too short: {}", height(STD));
        assert!(height(BIG) >= 6, "big too short: {}", height(BIG));
        assert!(
            height(OVERSIZED) >= 8,
            "oversized too short: {}",
            height(OVERSIZED)
        );
    }

    /// All assets must be pure ASCII so chars-count == cols.
    #[test]
    fn assets_are_pure_ascii() {
        for (name, asset) in [
            ("tiny", TINY),
            ("std", STD),
            ("big", BIG),
            ("oversized", OVERSIZED),
        ] {
            assert!(
                asset.is_ascii(),
                "{name}.txt contains non-ASCII bytes; width math would lie",
            );
        }
    }

    /// `lines()` must drop the trailing newline that
    /// `include_str!` leaves behind, otherwise the layout
    /// primitive grows a phantom empty row.
    #[test]
    fn lines_helper_strips_trailing_newline() {
        let l = lines(TINY);
        assert!(!l.is_empty());
        assert!(
            !l.last().unwrap().is_empty(),
            "lines helper did not strip trailing newline; last line was empty"
        );
    }

    /// :wq car must literally carry the spec-locked labels (Q1).
    #[test]
    fn big_car_carries_spec_labels() {
        assert!(BIG.contains("WRITE QUEUE"));
        assert!(BIG.contains("418 I'm an AI agent"));
    }

    /// Oversized car must carry the contract-significant labels from
    /// `docs/anim-large-art-contract.md`.
    #[test]
    fn oversized_carries_contract_labels() {
        assert!(OVERSIZED.contains("418"), "oversized missing 418 label");
        assert!(
            OVERSIZED.contains("AI AGENT"),
            "oversized missing AI AGENT label"
        );
        assert!(
            OVERSIZED.contains(":wq"),
            "oversized missing :wq trigger label"
        );
        assert!(
            OVERSIZED.contains("WRITE QUEUE"),
            "oversized missing WRITE QUEUE label"
        );
    }
}
