//! Animation primitives.
//!
//! Phase C landed the static asset layer ([`car`]); Phase F
//! incrementally adds the pure-function pieces that scenes will
//! orchestrate. So far this comprises the per-frame composer
//! ([`frame`]); the dumb-TTY fallback and crossterm-driven
//! renderer follow in subsequent commits.

pub mod car;
pub mod frame;
