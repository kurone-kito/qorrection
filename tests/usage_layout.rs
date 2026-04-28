//! Phase C5 snapshot tests for the usage screen.
//!
//! These pin every byte of the rendered usage screen across the
//! four width buckets. Snapshots are stored in
//! `tests/snapshots/usage_layout__*.snap` and reviewed with
//! `cargo insta review` whenever the layout, asset, or spec text
//! intentionally changes.

use qorrection::usage::render;

#[test]
fn usage_at_tiny_bucket() {
    insta::assert_snapshot!("usage_tiny_30", render(30));
}

#[test]
fn usage_at_small_bucket() {
    insta::assert_snapshot!("usage_small_60", render(60));
}

#[test]
fn usage_at_medium_bucket() {
    insta::assert_snapshot!("usage_medium_100", render(100));
}

#[test]
fn usage_at_large_bucket() {
    insta::assert_snapshot!("usage_large_140", render(140));
}

#[test]
fn usage_at_each_bucket_boundary() {
    // Just-inside boundary widths; if these drift, the
    // matching bucket-edge unit test in src/usage/screen.rs
    // already failed first.
    insta::assert_snapshot!("usage_boundary_39", render(39));
    insta::assert_snapshot!("usage_boundary_79", render(79));
    insta::assert_snapshot!("usage_boundary_119", render(119));
    insta::assert_snapshot!("usage_boundary_120", render(120));
}
