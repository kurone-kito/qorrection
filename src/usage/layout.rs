//! Pure side-by-side / single-column line composers.
//!
//! Both functions consume slices of `&str` lines (one per visual
//! row) and return an owned `String` ready to write to a TTY.
//! No ANSI sequences are emitted by this layer; the caller is
//! responsible for any styling. The functions are width-aware
//! only in terms of *visible columns* expressed as `usize`; they
//! deliberately do NOT parse east-asian-width or grapheme
//! clusters. The Phase C ASCII assets are pure 7-bit so this
//! restriction is acceptable for v0.1.

/// Compose a single-column block: each input line becomes one
/// output line, terminated with `\n`. Empty input returns an
/// empty string (no trailing newline).
///
/// This is essentially `lines.join("\n") + "\n"`, but the helper
/// exists so the Phase C4 entry point can dispatch through one
/// shape regardless of layout bucket.
pub fn render_single_column(lines: &[&str]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(lines.iter().map(|s| s.len() + 1).sum());
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Compose a two-column block. The left pane is right-padded
/// with spaces to `left_width` columns, joined to the right pane
/// by `gap` spaces. Whichever pane has fewer lines is padded
/// with empty lines so the output has `max(left.len, right.len)`
/// rows. Each row is terminated with `\n`. Trailing whitespace
/// at the end of each row (from a short right pane) is trimmed
/// because empty right cells should not carry the right pane's
/// padding.
///
/// `left_width` is the **visible-column budget** for the left
/// pane: each left line is padded out to that width, but if a
/// line is wider it is emitted as-is and the right pane shifts
/// over. This favors honesty over truncation; callers that need
/// strict-width output should pre-truncate.
pub fn render_two_column(left: &[&str], right: &[&str], left_width: usize, gap: usize) -> String {
    let rows = left.len().max(right.len());
    if rows == 0 {
        return String::new();
    }
    let gap_str: String = " ".repeat(gap);
    let mut out = String::new();
    for i in 0..rows {
        let l = left.get(i).copied().unwrap_or("");
        let r = right.get(i).copied().unwrap_or("");
        let l_visible = l.chars().count();
        out.push_str(l);
        if r.is_empty() {
            // Don't carry padding into an empty right cell;
            // strip trailing spaces from the left pane too so
            // the line ends cleanly.
            let trimmed_end = out.trim_end_matches(' ');
            out.truncate(trimmed_end.len());
        } else {
            if l_visible < left_width {
                for _ in 0..(left_width - l_visible) {
                    out.push(' ');
                }
            }
            out.push_str(&gap_str);
            out.push_str(r);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_column_empty_returns_empty() {
        assert_eq!(render_single_column(&[]), "");
    }

    #[test]
    fn single_column_joins_with_newlines() {
        assert_eq!(render_single_column(&["a", "b", "c"]), "a\nb\nc\n");
    }

    #[test]
    fn single_column_single_line_is_terminated() {
        assert_eq!(render_single_column(&["only"]), "only\n");
    }

    #[test]
    fn two_column_empty_inputs_return_empty() {
        assert_eq!(render_two_column(&[], &[], 10, 2), "");
    }

    #[test]
    fn two_column_balanced_rows_align() {
        let out = render_two_column(&["L1", "L2"], &["R1", "R2"], 4, 2);
        assert_eq!(out, "L1    R1\nL2    R2\n");
    }

    #[test]
    fn two_column_left_taller_pads_right_with_blanks() {
        let out = render_two_column(&["L1", "L2", "L3"], &["R1"], 4, 2);
        assert_eq!(out, "L1    R1\nL2\nL3\n");
    }

    #[test]
    fn two_column_right_taller_pads_left_with_blanks() {
        let out = render_two_column(&["L1"], &["R1", "R2", "R3"], 4, 2);
        assert_eq!(out, "L1    R1\n      R2\n      R3\n");
    }

    #[test]
    fn two_column_long_left_pushes_right_over() {
        let out = render_two_column(&["LONG_LEFT"], &["R"], 4, 2);
        assert_eq!(out, "LONG_LEFT  R\n");
    }

    #[test]
    fn two_column_zero_gap_is_allowed() {
        let out = render_two_column(&["L"], &["R"], 4, 0);
        assert_eq!(out, "L   R\n");
    }
}
