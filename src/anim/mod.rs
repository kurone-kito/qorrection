//! Animation primitives.
//!
//! Phase C landed the static asset layer ([`car`]); later phases
//! added the per-frame composer ([`frame`]), timeline builders
//! ([`scene`]), narrow-terminal text fallback ([`fallback`]),
//! the terminal-state RAII guard ([`terminal`]), and the
//! crossterm-driven presentation loop ([`render`]).

pub mod car;
pub mod fallback;
pub mod frame;
pub mod render;
pub mod scene;
pub mod terminal;
