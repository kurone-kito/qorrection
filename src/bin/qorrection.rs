//! qorrection -- PTY wrapper that intercepts Vim-style quit
//! commands and responds with playful animations.
//!
//! Thin shim over [`qorrection::run_from_env`]; behavior lives in
//! the library crate.

fn main() -> std::process::ExitCode {
    qorrection::run_from_env()
}
