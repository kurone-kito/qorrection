//! Trigger arming policy (allowlist of wrapped commands).
//!
//! Placeholder module — real implementation lands in Phase B/E
//! per the implementation plan. The locked spec requires
//! triggers to fire only when the basename of the wrapped command
//! (case-insensitive, with `.exe`/`.cmd`/`.bat` stripped) matches
//! one of:
//!
//! - `copilot`, `codex`, `claude`, `aichat`, `gemini`, `qwen`,
//!   `ollama`.
