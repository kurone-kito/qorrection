//! Usage screen rendering.
//!
//! Two layers, both already wired:
//!
//! 1. [`layout`] -- pure layout primitives. [`render_two_column`]
//!    joins a left and a right pane side-by-side, padding the
//!    shorter pane with blank lines; [`render_single_column`] is
//!    the narrow-terminal fallback.
//! 2. [`screen::render`] -- picks a
//!    [`crate::term::width::WidthBucket`] from the column count
//!    (single column for Tiny/Small, two column for Medium/Large
//!    once the right pane fits without wrap), selects an ASCII
//!    car asset and the v0.1 spec right-pane content, and
//!    delegates to layer 1.
//!
//! Everything here is pure: it takes `&str` slices and returns
//! owned `String`s. No I/O. The insta snapshots in
//! `tests/usage_layout.rs` feed synthetic widths via the public
//! API and assert the rendered bytes are byte-stable across runs.

mod layout;
mod screen;

pub use layout::{render_single_column, render_two_column};
pub use screen::render;
