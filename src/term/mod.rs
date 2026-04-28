//! Terminal capability inspection.
//!
//! Submodules in this directory each own one slice of the
//! "what kind of terminal are we attached to" question:
//!
//! - [`detect`] — TTY-ness, UTF-8 awareness, NO_COLOR, CI, dumb
//! - `width` — responsive layout buckets (Phase B3)
//! - `guard` — Drop-based raw-mode RAII (Phase B4)
//!
//! The crate-public surface lives here in `mod.rs` so callers
//! can `use qorrection::term::TerminalCaps;` regardless of how
//! the implementation is split.

pub mod detect;
pub mod guard;
pub mod width;

pub use detect::TerminalCaps;
pub use guard::{acquire as acquire_raw, RawGuard};
pub use width::{bucket, WidthBucket};
