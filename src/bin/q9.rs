//! `q9` -- short alias for the `qorrection` binary.
//!
//! Both binaries share the same library entry point so behavior
//! is identical regardless of `argv[0]`.

fn main() -> std::process::ExitCode {
    qorrection::run_from_env()
}
