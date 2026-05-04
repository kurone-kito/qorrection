//! Animation primitives.
//!
//! Phase C landed the static asset layer ([`car`]); Phase F adds
//! the pure-function pieces that scenes will orchestrate. This
//! includes the per-frame composer ([`frame`]) and the dumb-TTY
//! fallback ([`fallback`]); the crossterm-driven renderer follows
//! in a subsequent commit.

pub mod car;
pub mod fallback;
pub mod frame;
