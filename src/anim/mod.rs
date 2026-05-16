//! Animation primitives.
//!
//! Phase C landed the static asset layer ([`car`]); later phases
//! added the per-frame composer ([`frame`]), timeline builders
//! ([`scene`]), narrow-terminal text fallback ([`fallback`]),
//! and the terminal-state RAII guard ([`terminal`]) that the
//! upcoming crossterm renderer will use.

pub mod car;
pub mod fallback;
pub mod frame;
pub mod scene;
pub mod terminal;
