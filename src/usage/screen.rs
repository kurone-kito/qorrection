//! Render the fastfetch-style usage screen.
//!
//! This is the layer above [`super::layout`]: it picks the right
//! ASCII car asset and the right pane content for the current
//! [`WidthBucket`], then delegates to the layout primitive.
//!
//! The right pane (and the single-column variant for narrow
//! terminals) lists the v0.1 spec surface: synopsis, the three
//! triggers and their gags, the allowlist note, and the repo
//! URL. Content is byte-stable and pure ASCII so the Phase C5
//! insta snapshots can pin every column.

use crate::anim::car;
use crate::term::width::{bucket, WidthBucket};

/// Render the usage screen for `cols` terminal columns.
///
/// The output is plain text with `\n` line terminators; no ANSI
/// sequences are emitted. The caller (`run()`) is responsible
/// for writing it to stdout.
pub fn render(cols: u16) -> String {
    render_for_bucket(bucket(cols), cols)
}

/// Minimum total columns required to render the Medium two-column
/// layout without wrapping. Equals the Medium left-pane width
/// (30) + gap (2) + longest right-pane line (63 chars: the
/// description "qorrection: PTY wrapper that intercepts Vim-style
/// quit commands"). Below this width the Medium bucket would
/// still technically apply but the right pane wraps mid-sentence
/// and breaks alignment, so we fall back to the Small layout.
const MEDIUM_MIN_TWO_COL_WIDTH: u16 = 95;

fn render_for_bucket(b: WidthBucket, cols: u16) -> String {
    let right = right_pane();
    match b {
        WidthBucket::Tiny => {
            // Plain text only -- no ASCII car fits.
            super::layout::render_single_column(&right)
        }
        WidthBucket::Small => render_small(&right),
        WidthBucket::Medium => {
            if cols < MEDIUM_MIN_TWO_COL_WIDTH {
                // Right pane would wrap; degrade to the Small
                // single-column layout rather than corrupt the
                // alignment.
                render_small(&right)
            } else {
                let left = car::lines(car::STD);
                super::layout::render_two_column(&left, &right, 30, 2)
            }
        }
        WidthBucket::Large => {
            let left = car::lines(car::BIG);
            super::layout::render_two_column(&left, &right, 45, 3)
        }
    }
}

fn render_small(right: &[&str]) -> String {
    // Compact 1-line car header above the synopsis.
    let mut combined: Vec<&str> = vec!["[QQ] qorrection -- :q :wq :q!"];
    combined.extend_from_slice(right);
    super::layout::render_single_column(&combined)
}

/// The right pane content (and single-column body for narrow
/// terminals). Locked to spec §3 §R8.
fn right_pane() -> Vec<&'static str> {
    vec![
        concat!("qorrection ", env!("CARGO_PKG_VERSION")),
        "qorrection: PTY wrapper that intercepts Vim-style quit commands",
        "and dispatches FI-FO-FI-FO ambulance gags.",
        "",
        "USAGE:",
        "  q9 <cmd> [args...]",
        "  qorrection <cmd> [args...]",
        "",
        "TRIGGERS (active only when <cmd> is one of:",
        "  copilot, codex, claude, aichat, gemini, qwen, ollama):",
        "  :q   -> standard ambulance, FI-FO-FI-FO siren trail",
        "  :wq  -> larger ambulance carrying \"418 I'm an AI agent\"",
        "  :q!  -> 9-car parade (really wanted 9! = 362880)",
        "",
        "FLAGS:",
        "  -h, --help       show this screen",
        "  -V, --version    print 'qorrection X.Y.Z' and exit",
        "",
        "https://github.com/kurone-kito/qorrection",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_has_no_ascii_car() {
        let out = render(20);
        assert!(
            !out.contains("[QQ]"),
            "tiny should not include a car header"
        );
        assert!(!out.contains("(O)"), "tiny should not include car wheels");
        assert!(out.contains("USAGE:"));
    }

    #[test]
    fn small_has_compact_car_header() {
        let out = render(60);
        assert!(out.starts_with("[QQ] qorrection"));
        assert!(out.contains("USAGE:"));
        assert!(!out.contains("(O)"), "small should not draw the wheel art");
    }

    #[test]
    fn medium_uses_std_car() {
        let out = render(100);
        assert!(out.contains("(O)"), "medium should embed the std car");
        assert!(out.contains("USAGE:"));
        // Two-column → some lines start with the car art, not USAGE.
        assert!(out.lines().any(|l| l.starts_with("    _")));
    }

    #[test]
    fn large_uses_big_car_with_spec_labels() {
        let out = render(140);
        assert!(out.contains("WRITE QUEUE"));
        assert!(out.contains("418 I'm an AI agent"));
        assert!(out.contains("USAGE:"));
    }

    #[test]
    fn render_includes_version_string() {
        let out = render(100);
        assert!(out.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn output_is_pure_ascii() {
        for cols in [20u16, 60, 100, 140] {
            let out = render(cols);
            assert!(out.is_ascii(), "non-ASCII bytes at cols={cols}");
        }
    }

    #[test]
    fn bucket_boundaries_select_expected_layout() {
        // Just below each bucket boundary: must NOT have the
        // wider-bucket marker.
        assert!(!render(39).contains("[QQ] qorrection"));
        assert!(!render(79).contains("(O)"));
        assert!(!render(119).contains("WRITE QUEUE"));
    }

    #[test]
    fn medium_below_min_two_col_falls_back_to_small() {
        // 80 and 94 are inside the Medium bucket but cannot fit
        // the right pane next to the std car without wrapping.
        for cols in [80u16, 94] {
            let out = render(cols);
            assert!(
                !out.contains("(O)"),
                "cols={cols} should fall back to the Small layout"
            );
            assert!(
                out.starts_with("[QQ] qorrection"),
                "cols={cols} should use the Small compact car header"
            );
        }
    }

    #[test]
    fn medium_at_min_two_col_uses_std_car() {
        // 95 is the smallest column count that still fits the
        // longest right-pane line beside the 30-wide left pane
        // with a 2-space gap.
        let out = render(95);
        assert!(
            out.contains("(O)"),
            "cols=95 should render the std two-column car"
        );
    }

    #[test]
    fn no_rendered_line_exceeds_terminal_width_at_medium_or_above() {
        // Regression for the Medium-bucket wrap: every line we
        // emit at Medium / Large widths must fit within the
        // requested column count. (Tiny and Small are excluded:
        // those layouts emit the raw right pane verbatim and
        // their long-line behavior is a separate concern tracked
        // outside this fix.)
        for cols in [80u16, 94, 95, 100, 119, 120, 140] {
            let out = render(cols);
            for line in out.lines() {
                assert!(
                    line.chars().count() <= cols as usize,
                    "line {line:?} exceeds {cols} cols"
                );
            }
        }
    }
}
