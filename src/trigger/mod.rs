//! Trigger pipeline (Phase D).
//!
//! Pure byte-stream modules that decide whether the user just
//! typed a Vim-style quit literal at an interactive prompt:
//!
//! - [`paste`]     — bracketed-paste tracker (suppress while pasted)
//! - [`altscreen`] — alt-screen tracker on the **output** side
//!   (suppress while a TUI like vim is up)
//! - [`parser`]    — literal `:q` / `:wq` / `:q!` matcher
//!
//! Everything in this module is deterministic, side-effect free,
//! and unit-testable without a PTY. Phase E plugs the modules
//! into the real input pump and output arbiter.

pub mod altscreen;
pub mod parser;
pub mod paste;
