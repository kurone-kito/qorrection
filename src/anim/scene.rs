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

/// Build the larger `:wq` ambulance scene.
///
/// This timeline follows the same sweep / siren policy as [`q`]
/// but renders [`car::BIG`], preserving the spec-locked stacked
/// `WRITE QUEUE` / `418 I'm an AI agent` labels whenever the
/// full asset is visible.
pub fn wq(cols: u16) -> Vec<String> {
    sweep(car::BIG, cols)
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
}
