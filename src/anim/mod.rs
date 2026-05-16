//! Animation primitives.
//!
//! Phase C landed the static asset layer ([`car`]); later phases
//! added the per-frame composer ([`frame`]), timeline builders
//! ([`scene`]), narrow-terminal text fallback ([`fallback`]),
//! the terminal-state RAII guard ([`terminal`]), and the
//! crossterm-driven presentation loop ([`render`]).
//!
//! The post-v0.1 oversized cameo policy is documented in
//! `docs/anim-large-art-contract.md` so future scene or renderer
//! work can extend this layer without rewriting the current
//! ambulance behavior by accident.

pub mod car;
pub mod fallback;
pub mod frame;
pub mod render;
pub mod scene;
pub mod terminal;
