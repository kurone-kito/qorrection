//! Animation primitives.
//!
//! Phase C landed the static asset layer ([`car`]); Phase F adds
//! the pure-function pieces that scenes will orchestrate. This
//! includes the per-frame composer ([`frame`]), the dumb-TTY
//! fallback ([`fallback`]), and the first timeline builder
//! ([`scene`]); the crossterm-driven renderer follows in a
//! subsequent commit.

pub mod car;
pub mod fallback;
pub mod frame;
pub mod scene;
