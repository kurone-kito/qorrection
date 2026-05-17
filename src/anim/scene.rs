//! Pure animation timelines built from the lower-level frame
//! composer.
//!
//! Each scene returns a `Vec<String>` whose entries are complete
//! multi-line frames ready for the future crossterm renderer to
//! draw in order with a fixed tick delay. The scene layer owns
//! movement ranges, phase cadence, and right-edge clipping;
//! [`super::frame::frame`] deliberately stays oblivious to those
//! policy choices.

use crate::anim::{
    car,
    frame::{self, SirenPhase},
};

const BANG_CARS: usize = 9;

/// Build the standard `:q` ambulance scene.
///
/// The scene uses [`car::STD`] and sweeps it from the first
/// frame with any visible car pixels on the left to the last
/// frame with any visible car pixels on the right. Every frame
/// is clipped to `cols` columns so the scene stays within the
/// caller's terminal width budget. Successive frames flip the
/// siren phase (`Fi`, `Fo`, `Fi`, `Fo`, ...).
///
/// `cols == 0` returns an empty scene because there is no visible
/// canvas to animate within.
pub fn q(cols: u16) -> Vec<String> {
    sweep(car::STD, cols)
}

pub(crate) fn q_frame_count(cols: u16) -> usize {
    sweep_frame_count(car::STD, cols)
}

/// Build the compact 40-79 column ambulance scene.
///
/// This is the renderer's "small bucket" degrade path: when the
/// terminal is wide enough for art but too narrow for the standard
/// car, the animation keeps the same sweep / siren policy while
/// switching to [`car::TINY`].
pub fn tiny(cols: u16) -> Vec<String> {
    sweep(car::TINY, cols)
}

pub(crate) fn tiny_frame_count(cols: u16) -> usize {
    sweep_frame_count(car::TINY, cols)
}

/// Build the larger `:wq` ambulance scene.
///
/// This timeline follows the same sweep / siren policy as [`q`]
/// but renders [`car::BIG`], preserving the spec-locked stacked
/// `WRITE QUEUE` / `418 I'm an AI agent` labels whenever the
/// full asset is visible.
pub fn wq(cols: u16) -> Vec<String> {
    sweep(car::BIG, cols)
}

pub(crate) fn wq_frame_count(cols: u16) -> usize {
    sweep_frame_count(car::BIG, cols)
}

/// Maximum frames for the oversized `:wq` scene, per the contract in
/// `docs/anim-large-art-contract.md` (§ Timing Budget: ≤ 60 frames).
const OVERSIZED_MAX_FRAMES: usize = 60;

/// Build the oversized `:wq` scene for ≥ 160-col terminals.
///
/// Uses [`car::OVERSIZED`] swept via a sub-sampled timeline capped at
/// [`OVERSIZED_MAX_FRAMES`] frames so the animation stays within the
/// budget in `docs/anim-large-art-contract.md`. The step size grows
/// proportionally with terminal width so the cap always holds.
pub fn wq_oversized(cols: u16) -> Vec<String> {
    sweep_capped(car::OVERSIZED, cols, OVERSIZED_MAX_FRAMES)
}

pub(crate) fn wq_oversized_frame_count(cols: u16) -> usize {
    sweep_capped_frame_count(car::OVERSIZED, cols, OVERSIZED_MAX_FRAMES)
}

/// Build the `:q!` nine-car parade scene.
///
/// The parade reuses the standard `QUEUE` body nine times on a
/// shared convoy timeline. Unlike [`q`] and [`wq`], it omits the
/// FI/FO wheel-row trail: the single-car trail spans the whole
/// left canvas and would overwrite neighboring cars inside a
/// multi-car convoy.
pub fn bang(cols: u16) -> Vec<String> {
    parade(car::STD, cols, BANG_CARS)
}

pub(crate) fn bang_frame_count(cols: u16) -> usize {
    parade_frame_count(car::STD, cols, BANG_CARS)
}

/// Build the small-bucket `:q!` parade.
///
/// The renderer uses this when `40 <= cols < 80`: the trigger keeps
/// its parade identity, but the body shrinks to [`car::TINY`] so the
/// narrow viewport still gets a recognisably animated convoy.
pub fn tiny_bang(cols: u16) -> Vec<String> {
    parade(car::TINY, cols, BANG_CARS)
}

pub(crate) fn tiny_bang_frame_count(cols: u16) -> usize {
    parade_frame_count(car::TINY, cols, BANG_CARS)
}

/// Sweep one ASCII asset across the visible width using the
/// standard left-to-right timeline policy.
fn sweep(car_asset: &str, cols: u16) -> Vec<String> {
    if cols == 0 {
        return Vec::new();
    }

    let car_width = car::max_width(car_asset) as i32;
    let start_x = 1 - car_width;
    let end_x = i32::from(cols) - 1;
    let mut phase = SirenPhase::Fi;
    let mut frames = Vec::with_capacity((end_x - start_x + 1) as usize);

    for x in start_x..=end_x {
        let raw = frame::frame(car_asset, x, phase);
        frames.push(clip_right(&raw, cols as usize));
        phase = phase.flip();
    }

    frames
}

fn sweep_frame_count(car_asset: &str, cols: u16) -> usize {
    if cols == 0 {
        return 0;
    }

    let car_width = car::max_width(car_asset) as i32;
    let start_x = 1 - car_width;
    let end_x = i32::from(cols) - 1;
    (end_x - start_x + 1) as usize
}

/// Sweep one ASCII asset with a step size chosen to stay within
/// `max_frames`. When the full sweep fits within the budget, this
/// behaves identically to [`sweep`].
fn sweep_capped(car_asset: &str, cols: u16, max_frames: usize) -> Vec<String> {
    if cols == 0 || max_frames == 0 {
        return Vec::new();
    }
    let car_width = car::max_width(car_asset) as i32;
    let start_x = 1 - car_width;
    let end_x = i32::from(cols) - 1;
    let total = (end_x - start_x + 1) as usize;
    let step = total.div_ceil(max_frames).max(1);
    let mut phase = SirenPhase::Fi;
    let mut frames = Vec::with_capacity(total.div_ceil(step));
    let mut x = start_x;
    while x <= end_x {
        let raw = frame::frame(car_asset, x, phase);
        frames.push(clip_right(&raw, cols as usize));
        phase = phase.flip();
        x += step as i32;
    }
    frames
}

fn sweep_capped_frame_count(car_asset: &str, cols: u16, max_frames: usize) -> usize {
    if cols == 0 || max_frames == 0 {
        return 0;
    }
    let car_width = car::max_width(car_asset) as i32;
    let start_x = 1 - car_width;
    let end_x = i32::from(cols) - 1;
    let total = (end_x - start_x + 1) as usize;
    let step = total.div_ceil(max_frames).max(1);
    total.div_ceil(step)
}

/// Move a multi-car convoy across the visible width.
fn parade(car_asset: &str, cols: u16, cars: usize) -> Vec<String> {
    if cols == 0 || cars == 0 {
        return Vec::new();
    }

    let car_width = car::max_width(car_asset);
    let convoy_width = cars * car_width;
    let car_lines = car::lines(car_asset);
    let (car_left, car_right) = visible_bounds(&car_lines);
    let start_x = -(((convoy_width - car_width) + car_right) as i32);
    let end_x = i32::from(cols) - 1 - car_left as i32;
    let mut frames = Vec::with_capacity((end_x - start_x + 1) as usize);

    for convoy_x in start_x..=end_x {
        frames.push(render_parade_frame(
            &car_lines,
            cols as usize,
            convoy_x,
            car_width,
            cars,
        ));
    }

    frames
}

fn parade_frame_count(car_asset: &str, cols: u16, cars: usize) -> usize {
    if cols == 0 || cars == 0 {
        return 0;
    }

    let car_width = car::max_width(car_asset);
    let convoy_width = cars * car_width;
    let car_lines = car::lines(car_asset);
    let (car_left, car_right) = visible_bounds(&car_lines);
    let start_x = -(((convoy_width - car_width) + car_right) as i32);
    let end_x = i32::from(cols) - 1 - car_left as i32;
    (end_x - start_x + 1) as usize
}

/// Find the leftmost and rightmost non-space columns in one asset.
fn visible_bounds(car_lines: &[&str]) -> (usize, usize) {
    let mut left = usize::MAX;
    let mut right = 0;

    for line in car_lines {
        let bytes = line.as_bytes();
        if let Some(first) = bytes.iter().position(|byte| *byte != b' ') {
            left = left.min(first);
            right = right.max(
                bytes
                    .iter()
                    .rposition(|byte| *byte != b' ')
                    .expect("non-empty ASCII asset line must have a right edge"),
            );
        }
    }

    if left == usize::MAX {
        (0, 0)
    } else {
        (left, right)
    }
}

/// Render one parade frame directly into the visible-width canvas.
fn render_parade_frame(
    car_lines: &[&str],
    cols: usize,
    convoy_x: i32,
    car_width: usize,
    cars: usize,
) -> String {
    let mut rows = vec![vec![b' '; cols]; car_lines.len()];

    for car_idx in 0..cars {
        let car_x = convoy_x + (car_idx * car_width) as i32;
        paint_visible_car(&mut rows, car_lines, car_x);
    }

    join_rows(rows)
}

/// Paint one car body into the visible canvas without any siren trail.
fn paint_visible_car(rows: &mut [Vec<u8>], car_lines: &[&str], x_offset: i32) {
    for (row, line) in rows.iter_mut().zip(car_lines.iter().copied()) {
        let bytes = line.as_bytes();
        let (src_start, dst_start) = if x_offset >= 0 {
            (0, x_offset as usize)
        } else {
            (x_offset.unsigned_abs() as usize, 0)
        };

        if src_start >= bytes.len() || dst_start >= row.len() {
            continue;
        }

        let copy_len = (row.len() - dst_start).min(bytes.len() - src_start);
        for idx in 0..copy_len {
            let byte = bytes[src_start + idx];
            if byte != b' ' {
                row[dst_start + idx] = byte;
            }
        }
    }
}

/// Join visible rows while trimming trailing padding spaces.
fn join_rows(rows: Vec<Vec<u8>>) -> String {
    let mut out = String::new();

    for (idx, row) in rows.iter().enumerate() {
        let end = row
            .iter()
            .rposition(|byte| *byte != b' ')
            .map_or(0, |i| i + 1);
        out.push_str(std::str::from_utf8(&row[..end]).expect("parade scene rows must stay ASCII"));
        if idx + 1 < rows.len() {
            out.push('\n');
        }
    }

    out
}

/// Clip each row in an ASCII frame to the visible width.
///
/// `frame::frame()` owns left-edge geometry only; scene builders
/// cap the right edge to the terminal width so the future
/// renderer can print each frame directly without overflowing the
/// viewport.
fn clip_right(frame: &str, cols: usize) -> String {
    debug_assert!(frame.is_ascii(), "scene frames must stay ASCII");

    let lines: Vec<&str> = frame.split('\n').collect();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let visible = cols.min(line.len());
        out.push_str(&line[..visible]);
        if i + 1 < lines.len() {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label_row(frame: &str) -> &str {
        frame.split('\n').nth(2).unwrap()
    }

    fn count_occurrences(haystack: &str, needle: &str) -> usize {
        let mut count = 0;
        let mut rest = haystack;
        while let Some(idx) = rest.find(needle) {
            count += 1;
            rest = &rest[idx + needle.len()..];
        }
        count
    }

    fn positions(haystack: &str, needle: &str) -> Vec<usize> {
        let mut found = Vec::new();
        let mut start = 0;
        while let Some(idx) = haystack[start..].find(needle) {
            let pos = start + idx;
            found.push(pos);
            start = pos + needle.len();
        }
        found
    }

    fn bang_x_zero_idx() -> usize {
        let lines = car::lines(car::STD);
        let (_, car_right) = visible_bounds(&lines);
        ((BANG_CARS - 1) * car::max_width(car::STD)) + car_right
    }

    fn wheel_row(frame: &str) -> &str {
        frame.rsplit('\n').next().unwrap()
    }

    #[test]
    fn q_scene_returns_empty_when_no_columns_are_visible() {
        assert!(q(0).is_empty());
    }

    #[test]
    fn q_scene_uses_the_full_visible_sweep_range() {
        let cols = 80;
        let scene = q(cols);
        let expected = cols as usize + car::max_width(car::STD) - 1;
        assert_eq!(scene.len(), expected);
        assert!(
            scene
                .first()
                .unwrap()
                .chars()
                .any(|c| c != ' ' && c != '\n'),
            "first frame should contain the first visible car sliver",
        );
        assert!(
            scene.last().unwrap().chars().any(|c| c != ' ' && c != '\n'),
            "last frame should contain the last visible car sliver",
        );
    }

    #[test]
    fn q_scene_contains_the_full_std_car_when_x_reaches_zero() {
        let scene = q(80);
        let x_zero_idx = car::max_width(car::STD) - 1;
        assert_eq!(scene[x_zero_idx], frame::frame(car::STD, 0, SirenPhase::Fi));
    }

    #[test]
    fn q_scene_right_clips_every_row_to_the_requested_width() {
        let cols = 10;
        let scene = q(cols);
        for frame in &scene {
            let lines: Vec<&str> = frame.split('\n').collect();
            assert_eq!(lines.len(), car::height(car::STD));
            for line in lines {
                assert!(
                    line.len() <= cols as usize,
                    "line exceeds {cols} cols: {line:?}",
                );
            }
        }
    }

    #[test]
    fn q_scene_alternates_the_siren_phase_on_successive_positive_offsets() {
        let scene = q(80);
        let car_width = car::max_width(car::STD);
        let a = wheel_row(&scene[car_width + 5]);
        let b = wheel_row(&scene[car_width + 6]);

        if a.starts_with("Fi-Fo-") {
            assert!(
                b.starts_with("Fo-Fi-F"),
                "next frame should flip to Fo-leading trail; got {b:?}",
            );
        } else {
            assert!(
                a.starts_with("Fo-Fi-"),
                "expected Fi/Fo lead in positive-offset frame, got {a:?}",
            );
            assert!(
                b.starts_with("Fi-Fo-F"),
                "next frame should flip back to Fi-leading trail; got {b:?}",
            );
        }
    }

    #[test]
    fn q_scene_never_uses_the_big_418_asset() {
        let scene = q(120);
        assert!(scene
            .iter()
            .all(|frame| !frame.contains("418 I'm an AI agent")));
    }

    #[test]
    fn wq_scene_returns_empty_when_no_columns_are_visible() {
        assert!(wq(0).is_empty());
    }

    #[test]
    fn wq_scene_uses_the_full_visible_sweep_range() {
        let cols = 140;
        let scene = wq(cols);
        let expected = cols as usize + car::max_width(car::BIG) - 1;
        assert_eq!(scene.len(), expected);
        assert!(
            scene
                .first()
                .unwrap()
                .chars()
                .any(|c| c != ' ' && c != '\n'),
            "first frame should contain the first visible car sliver",
        );
        assert!(
            scene.last().unwrap().chars().any(|c| c != ' ' && c != '\n'),
            "last frame should contain the last visible car sliver",
        );
    }

    #[test]
    fn wq_scene_contains_the_full_big_car_when_x_reaches_zero() {
        let scene = wq(160);
        let x_zero_idx = car::max_width(car::BIG) - 1;
        assert_eq!(scene[x_zero_idx], frame::frame(car::BIG, 0, SirenPhase::Fi));
        assert!(scene[x_zero_idx].contains("WRITE QUEUE"));
        assert!(scene[x_zero_idx].contains("418 I'm an AI agent"));
    }

    #[test]
    fn wq_scene_right_clips_every_row_to_the_requested_width() {
        let cols = 20;
        let scene = wq(cols);
        for frame in &scene {
            let lines: Vec<&str> = frame.split('\n').collect();
            assert_eq!(lines.len(), car::height(car::BIG));
            for line in lines {
                assert!(
                    line.len() <= cols as usize,
                    "line exceeds {cols} cols: {line:?}",
                );
            }
        }
    }

    #[test]
    fn wq_scene_alternates_the_siren_phase_on_successive_positive_offsets() {
        let scene = wq(160);
        let car_width = car::max_width(car::BIG);
        let a = wheel_row(&scene[car_width + 5]);
        let b = wheel_row(&scene[car_width + 6]);

        if a.starts_with("Fi-Fo-") {
            assert!(
                b.starts_with("Fo-Fi-F"),
                "next frame should flip to Fo-leading trail; got {b:?}",
            );
        } else {
            assert!(
                a.starts_with("Fo-Fi-"),
                "expected Fi/Fo lead in positive-offset frame, got {a:?}",
            );
            assert!(
                b.starts_with("Fi-Fo-F"),
                "next frame should flip back to Fi-leading trail; got {b:?}",
            );
        }
    }

    #[test]
    fn bang_scene_returns_empty_when_no_columns_are_visible() {
        assert!(bang(0).is_empty());
    }

    #[test]
    fn bang_scene_uses_the_full_visible_convoy_sweep_range() {
        let cols = 80;
        let scene = bang(cols);
        let std_lines = car::lines(car::STD);
        let (car_left, car_right) = visible_bounds(&std_lines);
        let visible_width =
            ((BANG_CARS - 1) * car::max_width(car::STD)) + (car_right - car_left + 1);
        let expected = cols as usize + visible_width - 1;
        assert_eq!(scene.len(), expected);
        assert!(
            scene
                .first()
                .unwrap()
                .chars()
                .any(|c| c != ' ' && c != '\n'),
            "first frame should contain the first visible convoy sliver",
        );
        assert!(
            scene.last().unwrap().chars().any(|c| c != ' ' && c != '\n'),
            "last frame should contain the last visible convoy sliver",
        );
    }

    #[test]
    fn bang_scene_contains_all_nine_queue_labels_when_fully_visible() {
        let convoy_width = BANG_CARS * car::max_width(car::STD);
        let scene = bang(convoy_width as u16);
        let x_zero_idx = bang_x_zero_idx();
        let frame = &scene[x_zero_idx];
        let label_positions = positions(label_row(frame), "QUEUE");

        assert_eq!(count_occurrences(frame, "QUEUE"), BANG_CARS);
        assert_eq!(label_positions.len(), BANG_CARS);
        for pair in label_positions.windows(2) {
            assert_eq!(
                pair[1] - pair[0],
                car::max_width(car::STD),
                "adjacent labels should stay convoy-width apart",
            );
        }
    }

    #[test]
    fn bang_scene_right_clips_every_row_to_the_requested_width() {
        let cols = 24;
        let scene = bang(cols);
        for frame in &scene {
            let lines: Vec<&str> = frame.split('\n').collect();
            assert_eq!(lines.len(), car::height(car::STD));
            for line in lines {
                assert!(
                    line.len() <= cols as usize,
                    "line exceeds {cols} cols: {line:?}",
                );
            }
        }
    }

    #[test]
    fn bang_scene_moves_the_convoy_one_column_per_tick() {
        let convoy_width = BANG_CARS * car::max_width(car::STD);
        let scene = bang(convoy_width as u16 + 1);
        let x_zero_idx = bang_x_zero_idx();
        let at_zero = positions(label_row(&scene[x_zero_idx]), "QUEUE");
        let at_one = positions(label_row(&scene[x_zero_idx + 1]), "QUEUE");

        assert_eq!(at_zero.len(), BANG_CARS);
        assert_eq!(at_one.len(), BANG_CARS);
        for (left, right) in at_zero.iter().zip(at_one.iter()) {
            assert_eq!(*right, *left + 1, "convoy should advance by one column");
        }
    }

    #[test]
    fn bang_scene_never_uses_the_big_418_asset() {
        let scene = bang(200);
        assert!(scene
            .iter()
            .all(|frame| !frame.contains("418 I'm an AI agent")));
    }
}
