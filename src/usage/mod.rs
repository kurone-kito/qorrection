//! Usage screen rendering.
//!
//! Phase C builds this in two layers:
//!
//! 1. A pure layout primitive ([`render_two_column`]) that joins
//!    a left and a right pane side-by-side, padding the shorter
//!    pane with blank lines. This module currently contains only
//!    layer 1 — Phase C4 will wire the actual usage content
//!    (synopsis, triggers, repo URL) on top.
//! 2. The `usage` entry point that picks a [`crate::term::width::WidthBucket`]
//!    (single column for Tiny/Small, two column for Medium/Large)
//!    and dispatches to the layout primitive.
//!
//! Everything here is pure: it takes `&str` slices and returns
//! owned `String`s. No I/O. The Phase C5 insta snapshots feed
//! synthetic widths via the public API and assert the rendered
//! bytes are byte-stable across runs.

mod layout;

pub use layout::{render_single_column, render_two_column};
