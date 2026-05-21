//! Trigger pipeline (Phase D).
//!
//! Pure byte-stream modules that decide whether the user just
//! typed a Vim-style quit literal at an interactive prompt:
//!
//! - [`input`]     -- combined input-side pump state machine
//! - [`output`]    -- child-output arbiter that updates alt-screen state
//! - [`paste`]     -- bracketed-paste tracker (suppress while pasted)
//! - [`altscreen`] -- alt-screen tracker on the **output** side
//!   (suppress while a TUI like vim is up)
//! - [`tui_activity`] -- output-side heuristic for children that
//!   draw a TUI without entering the alternate screen (Codex CLI
//!   et al.); supplements the alt-screen tracker so the animation
//!   renderer can fall back to a non-overlay gag in that case
//! - [`parser`]    -- literal `:q` / `:wq` / `:q!` matcher
//!
//! Everything in this module is deterministic, side-effect free,
//! and unit-testable without a PTY. Phase E plugs the modules
//! into the real input pump and output arbiter.

pub mod altscreen;
pub mod input;
pub mod output;
pub mod parser;
pub mod paste;
pub mod tui_activity;
