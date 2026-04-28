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
    render_for_bucket(bucket(cols))
}

fn render_for_bucket(b: WidthBucket) -> String {
    let right = right_pane();
    match b {
        WidthBucket::Tiny => {
            // Plain text only — no ASCII car fits.
            super::layout::render_single_column(&right)
        }
        WidthBucket::Small => {
            // Compact 1-line car header above the synopsis.
            let mut combined: Vec<&str> = vec!["[QQ] qorrection -- :q :wq :q!"];
            combined.extend_from_slice(&right);
            super::layout::render_single_column(&combined)
        }
        WidthBucket::Medium => {
            let left = car::lines(car::STD);
            super::layout::render_two_column(&left, &right, 30, 2)
        }
        WidthBucket::Large => {
            let left = car::lines(car::BIG);
            super::layout::render_two_column(&left, &right, 45, 3)
        }
    }
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
}
