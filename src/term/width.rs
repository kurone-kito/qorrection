//! Responsive layout width buckets.
//!
//! Animations and the usage screen pick assets and layouts by
//! the bucket of the current terminal width, not by the raw
//! column count. Locking the four buckets in one place keeps
//! Phase C's usage screen and Phase F's animations from
//! independently inventing thresholds that drift out of sync.
//!
//! Boundaries:
//!
//! | Columns        | Bucket                        |
//! | -------------- | ----------------------------- |
//! | `< 40`         | [`WidthBucket::Tiny`]         |
//! | `40..=79`      | [`WidthBucket::Small`]        |
//! | `80..=119`     | [`WidthBucket::Medium`]       |
//! | `120..=159`    | [`WidthBucket::Large`]        |
//! | `>= 160`       | [`WidthBucket::Oversized`]    |

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WidthBucket {
    /// `< 40` columns. Triggers the plain-text fallback gag --
    /// no ASCII car art fits here.
    Tiny,
    /// `40..=79` columns. Compact one-line car header.
    Small,
    /// `80..=119` columns. Standard car for `:q`.
    Medium,
    /// `120..=159` columns. Big car (used by `:wq` for the two-line
    /// label) and the widest usage layout.
    Large,
    /// `>= 160` columns. Oversized cameo track for `:wq` per the
    /// contract in `docs/anim-large-art-contract.md`.
    Oversized,
}

/// Map a raw terminal column count to its bucket.
///
/// `0` is clamped to [`WidthBucket::Tiny`] so a misreported
/// width never panics or selects a wider asset than fits.
pub fn bucket(cols: u16) -> WidthBucket {
    match cols {
        0..=39 => WidthBucket::Tiny,
        40..=79 => WidthBucket::Small,
        80..=119 => WidthBucket::Medium,
        120..=159 => WidthBucket::Large,
        _ => WidthBucket::Oversized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_tiny() {
        assert_eq!(bucket(0), WidthBucket::Tiny);
    }

    #[test]
    fn boundary_39_is_tiny() {
        assert_eq!(bucket(39), WidthBucket::Tiny);
    }

    #[test]
    fn boundary_40_is_small() {
        assert_eq!(bucket(40), WidthBucket::Small);
    }

    #[test]
    fn boundary_79_is_small() {
        assert_eq!(bucket(79), WidthBucket::Small);
    }

    #[test]
    fn boundary_80_is_medium() {
        assert_eq!(bucket(80), WidthBucket::Medium);
    }

    #[test]
    fn boundary_119_is_medium() {
        assert_eq!(bucket(119), WidthBucket::Medium);
    }

    #[test]
    fn boundary_120_is_large() {
        assert_eq!(bucket(120), WidthBucket::Large);
    }

    #[test]
    fn boundary_159_is_large() {
        assert_eq!(bucket(159), WidthBucket::Large);
    }

    #[test]
    fn boundary_160_is_oversized() {
        assert_eq!(bucket(160), WidthBucket::Oversized);
    }

    #[test]
    fn very_wide_terminal_is_oversized() {
        assert_eq!(bucket(500), WidthBucket::Oversized);
        assert_eq!(bucket(u16::MAX), WidthBucket::Oversized);
    }

    #[test]
    fn buckets_are_ordered_by_width() {
        // Lock the Ord derivation so callers can compare
        // buckets without re-listing every variant.
        assert!(WidthBucket::Tiny < WidthBucket::Small);
        assert!(WidthBucket::Small < WidthBucket::Medium);
        assert!(WidthBucket::Medium < WidthBucket::Large);
        assert!(WidthBucket::Large < WidthBucket::Oversized);
    }
}
