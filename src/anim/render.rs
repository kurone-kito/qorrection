//! crossterm-driven animation renderer.
//!
//! The public [`render`] entry point owns the side-effecting
//! presentation loop: choose the appropriate scene for the fired
//! trigger and current width bucket, enter the alternate screen via
//! [`super::terminal::TerminalGuard`], draw each frame at the home
//! cursor position, sleep a fixed tick, and rely on guard drop for
//! restoration on every exit path.

use std::{io::Write, time::Duration};

use crate::{
    anim::{fallback, scene, terminal},
    term::width::{bucket, WidthBucket},
    trigger::parser::Outcome,
    Result,
};

/// Fixed per-frame hold time.
///
/// The renderer sleeps after every successful draw, including the
/// final frame, so the last visible gag remains on screen briefly
/// before [`terminal::TerminalGuard`] restores the original screen.
pub const FRAME_DELAY: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanKind {
    Fallback,
    Tiny,
    TinyBang,
    Q,
    Wq,
    Bang,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderPlan {
    pub(crate) kind: PlanKind,
    pub(crate) frames: Vec<String>,
}

/// Render one fired trigger's animation for the given terminal width.
pub fn render(outcome: Outcome, cols: u16) -> Result<()> {
    let mut out = std::io::stdout();
    render_with(
        outcome,
        cols,
        terminal::acquire,
        |frame| draw_frame(&mut out, frame),
        std::thread::sleep,
    )
}

/// Test seam covering plan selection, terminal-guard lifetime, and
/// draw/sleep ordering without touching the real terminal.
pub(crate) fn render_with<A, D, S>(
    outcome: Outcome,
    cols: u16,
    acquire: A,
    mut draw: D,
    mut sleep: S,
) -> Result<()>
where
    A: FnOnce() -> Result<terminal::TerminalGuard>,
    D: FnMut(&str) -> Result<()>,
    S: FnMut(Duration),
{
    let Some(plan) = render_plan(outcome, cols) else {
        return Ok(());
    };
    if plan.frames.is_empty() {
        return Ok(());
    }

    let _guard = acquire()?;
    for frame in &plan.frames {
        draw(frame)?;
        sleep(FRAME_DELAY);
    }
    Ok(())
}

/// Build the pure render plan for one trigger + width combination.
///
/// Degrade policy is centralized here so boundary tests can pin it:
///
/// - `< 40` columns → one-line trigger-specific fallback gag
/// - `40..=79`      → tiny ambulance, except `:q!` keeps a tiny parade
/// - `80..=119`     → standard `:q` scene; `:wq` degrades here because
///   the big 418-labeled asset does not fit
/// - `>= 120`       → `:wq` upgrades to the big labeled scene, `:q!`
///   keeps the parade, `:q` stays on the standard car
pub(crate) fn render_plan(outcome: Outcome, cols: u16) -> Option<RenderPlan> {
    let b = bucket(cols);
    match outcome {
        Outcome::None => None,
        Outcome::Q => Some(match b {
            WidthBucket::Tiny => fallback_plan(fallback::Trigger::Q),
            WidthBucket::Small => RenderPlan {
                kind: PlanKind::Tiny,
                frames: scene::tiny(cols),
            },
            WidthBucket::Medium | WidthBucket::Large => RenderPlan {
                kind: PlanKind::Q,
                frames: scene::q(cols),
            },
        }),
        Outcome::Wq => Some(match b {
            WidthBucket::Tiny => fallback_plan(fallback::Trigger::Wq),
            WidthBucket::Small => RenderPlan {
                kind: PlanKind::Tiny,
                frames: scene::tiny(cols),
            },
            WidthBucket::Medium => RenderPlan {
                kind: PlanKind::Q,
                frames: scene::q(cols),
            },
            WidthBucket::Large => RenderPlan {
                kind: PlanKind::Wq,
                frames: scene::wq(cols),
            },
        }),
        Outcome::QBang => Some(match b {
            WidthBucket::Tiny => fallback_plan(fallback::Trigger::Bang),
            WidthBucket::Small => RenderPlan {
                kind: PlanKind::TinyBang,
                frames: scene::tiny_bang(cols),
            },
            WidthBucket::Medium | WidthBucket::Large => RenderPlan {
                kind: PlanKind::Bang,
                frames: scene::bang(cols),
            },
        }),
    }
}

fn fallback_plan(trigger: fallback::Trigger) -> RenderPlan {
    RenderPlan {
        kind: PlanKind::Fallback,
        frames: vec![fallback::fallback(trigger).to_string()],
    }
}

fn draw_frame<W>(out: &mut W, frame: &str) -> Result<()>
where
    W: Write,
{
    crossterm::queue!(
        out,
        crossterm::cursor::MoveTo(0, 0),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
    )?;
    write!(out, "{frame}")?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        panic::AssertUnwindSafe,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };

    #[test]
    fn render_plan_returns_none_when_no_trigger_fired() {
        assert_eq!(render_plan(Outcome::None, 120), None);
    }

    #[test]
    fn render_plan_uses_fallback_below_forty_columns() {
        let q = render_plan(Outcome::Q, 39).unwrap();
        let wq = render_plan(Outcome::Wq, 0).unwrap();
        let bang = render_plan(Outcome::QBang, 12).unwrap();

        assert_eq!(q.kind, PlanKind::Fallback);
        assert_eq!(
            q.frames,
            vec![fallback::fallback(fallback::Trigger::Q).to_string()]
        );
        assert_eq!(wq.kind, PlanKind::Fallback);
        assert_eq!(
            wq.frames,
            vec![fallback::fallback(fallback::Trigger::Wq).to_string()]
        );
        assert_eq!(bang.kind, PlanKind::Fallback);
        assert_eq!(
            bang.frames,
            vec![fallback::fallback(fallback::Trigger::Bang).to_string()]
        );
    }

    #[test]
    fn render_plan_selects_expected_bucket_variants() {
        assert_eq!(render_plan(Outcome::Q, 40).unwrap().kind, PlanKind::Tiny);
        assert_eq!(render_plan(Outcome::Q, 79).unwrap().kind, PlanKind::Tiny);
        assert_eq!(render_plan(Outcome::Q, 80).unwrap().kind, PlanKind::Q);
        assert_eq!(render_plan(Outcome::Q, 140).unwrap().kind, PlanKind::Q);

        assert_eq!(render_plan(Outcome::Wq, 40).unwrap().kind, PlanKind::Tiny);
        assert_eq!(render_plan(Outcome::Wq, 119).unwrap().kind, PlanKind::Q);
        assert_eq!(render_plan(Outcome::Wq, 120).unwrap().kind, PlanKind::Wq);

        assert_eq!(
            render_plan(Outcome::QBang, 79).unwrap().kind,
            PlanKind::TinyBang
        );
        assert_eq!(
            render_plan(Outcome::QBang, 80).unwrap().kind,
            PlanKind::Bang
        );
        assert_eq!(
            render_plan(Outcome::QBang, 140).unwrap().kind,
            PlanKind::Bang
        );
    }

    #[test]
    fn render_plan_small_bucket_uses_tiny_timelines() {
        let q = render_plan(Outcome::Q, 40).unwrap();
        let bang = render_plan(Outcome::QBang, 40).unwrap();

        assert_eq!(q.frames, scene::tiny(40));
        assert_eq!(bang.frames, scene::tiny_bang(40));
    }

    #[test]
    fn render_plan_medium_wq_degrades_to_the_standard_scene() {
        let plan = render_plan(Outcome::Wq, 119).unwrap();

        assert_eq!(plan.kind, PlanKind::Q);
        assert_eq!(plan.frames, scene::q(119));
        assert!(plan
            .frames
            .iter()
            .all(|frame| !frame.contains("418 I'm an AI agent")));
    }

    #[test]
    fn render_plan_large_wq_preserves_the_big_418_scene() {
        let plan = render_plan(Outcome::Wq, 120).unwrap();

        assert_eq!(plan.kind, PlanKind::Wq);
        assert_eq!(plan.frames, scene::wq(120));
        assert!(plan
            .frames
            .iter()
            .any(|frame| frame.contains("418 I'm an AI agent")));
    }

    #[test]
    fn render_with_none_is_a_no_op() {
        let draws = Arc::new(AtomicUsize::new(0));
        let sleeps = Arc::new(AtomicUsize::new(0));
        let acquires = Arc::new(AtomicUsize::new(0));

        let draw_counter = Arc::clone(&draws);
        let sleep_counter = Arc::clone(&sleeps);
        let acquire_counter = Arc::clone(&acquires);

        render_with(
            Outcome::None,
            120,
            move || {
                acquire_counter.fetch_add(1, Ordering::SeqCst);
                Ok(terminal::TerminalGuard::noop())
            },
            move |_frame| {
                draw_counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |_delay| {
                sleep_counter.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();

        assert_eq!(acquires.load(Ordering::SeqCst), 0);
        assert_eq!(draws.load(Ordering::SeqCst), 0);
        assert_eq!(sleeps.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn render_with_draws_sleeps_and_restores_for_single_frame_fallback() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let acquire_events = Arc::clone(&events);
        let draw_events = Arc::clone(&events);
        let sleep_events = Arc::clone(&events);

        render_with(
            Outcome::Q,
            39,
            move || {
                Ok(terminal::TerminalGuard::with_restore_hook(move || {
                    acquire_events.lock().unwrap().push("restore".to_string());
                }))
            },
            move |frame| {
                draw_events.lock().unwrap().push(format!("draw:{frame}"));
                Ok(())
            },
            move |delay| {
                sleep_events
                    .lock()
                    .unwrap()
                    .push(format!("sleep:{delay:?}"));
            },
        )
        .unwrap();

        let events = events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![
                format!("draw:{}", fallback::fallback(fallback::Trigger::Q)),
                format!("sleep:{FRAME_DELAY:?}"),
                "restore".to_string(),
            ]
        );
    }

    #[test]
    fn render_with_sleeps_once_per_drawn_frame() {
        let draws = Arc::new(AtomicUsize::new(0));
        let sleeps = Arc::new(AtomicUsize::new(0));

        let draw_counter = Arc::clone(&draws);
        let sleep_counter = Arc::clone(&sleeps);

        render_with(
            Outcome::Q,
            40,
            || Ok(terminal::TerminalGuard::noop()),
            move |_frame| {
                draw_counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |_delay| {
                sleep_counter.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();

        let expected = scene::tiny(40).len();
        assert_eq!(draws.load(Ordering::SeqCst), expected);
        assert_eq!(sleeps.load(Ordering::SeqCst), expected);
    }

    #[test]
    fn render_with_restores_when_draw_fails_after_acquire() {
        let restores = Arc::new(AtomicUsize::new(0));
        let restore_counter = Arc::clone(&restores);

        let err = render_with(
            Outcome::Q,
            39,
            move || {
                Ok(terminal::TerminalGuard::with_restore_hook(move || {
                    restore_counter.fetch_add(1, Ordering::SeqCst);
                }))
            },
            |_frame| Err(std::io::Error::other("draw failed").into()),
            |_delay| {},
        )
        .unwrap_err();

        assert!(matches!(err, crate::Error::Terminal(_)));
        assert_eq!(restores.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn render_with_restores_when_draw_panics() {
        let restores = Arc::new(AtomicUsize::new(0));
        let restore_counter = Arc::clone(&restores);

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = render_with(
                Outcome::Q,
                39,
                move || {
                    Ok(terminal::TerminalGuard::with_restore_hook(move || {
                        restore_counter.fetch_add(1, Ordering::SeqCst);
                    }))
                },
                |_frame| -> Result<()> {
                    panic!("boom");
                },
                |_delay| {},
            );
        }));

        assert!(result.is_err());
        assert_eq!(restores.load(Ordering::SeqCst), 1);
    }
}
