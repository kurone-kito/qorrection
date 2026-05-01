//! Map [`portable_pty::ExitStatus`] to [`std::process::ExitCode`].
//!
//! `portable-pty` reports the wrapped child's outcome via
//! [`portable_pty::ExitStatus`], which carries either an exit
//! code (`u32`) or a *locale-dependent* signal name string
//! produced by `libc::strsignal()` on Unix
//! (or the literal `"Signal N"` fallback when `strsignal`
//! returns null). Our public boundary type
//! [`crate::Error::Signal`] needs the raw signal *number*, not
//! a locale-dependent string. This module isolates that mapping
//! so the supervisor in [`super::session`] stays a pure I/O
//! state machine.
//!
//! ## Recovering the signum from the signal name
//!
//! Three classification layers, tried in order:
//!
//! 1. **`#[cfg(unix)]` dynamic same-locale reverse lookup.**
//!    Iterate `1..=64` and compare
//!    `unsafe { libc::strsignal(n) }` (rendered as a `String`
//!    via `CStr`) against the reported name. Because
//!    `portable-pty` itself produced the name with the *same*
//!    `strsignal` in the *same* process locale, this round-trips
//!    deterministically when the running locale is consistent
//!    (the common case).
//! 2. **English POSIX-name allowlist.** Some distributions or
//!    embedded contexts may have already cached a name from a
//!    different locale (e.g. via a thread that switched locale,
//!    or a pre-baked test status). The static allowlist covers
//!    every signal whose POSIX name fits in a small table
//!    (HUP, INT, …, SYS).
//! 3. **`"Signal N"` numeric parse.** The literal fallback
//!    portable-pty emits when `strsignal` returns null.
//!
//! Only the genuinely unreachable defensive branch falls through
//! to `signum = 0` — see [`crate::Error::exit_code`] which maps
//! that to exit `128` (POSIX "killed by signal 0", a defined
//! sentinel rather than wrapping or panicking).
//!
//! ## Test layering
//!
//! - **Unit** (this file + [`crate::error`]): mock
//!   [`portable_pty::ExitStatus`] values cover every recovery
//!   layer and the saturating `128 + signum` arithmetic.
//! - **Integration / E2E** (`tests/pty_e2e.rs`,
//!   `tests/nontty_passthrough.rs`): drive the shipped `q9`
//!   binary through both wrap branches with `kill -15 $$` and
//!   assert exit `143`, pinning the full chain
//!   (`portable-pty` reporting → [`map_exit_status`] →
//!   [`crate::Error::Signal`] → [`std::process::ExitCode`]).

use std::process::ExitCode;

use portable_pty::ExitStatus;

use crate::{Error, Result};

/// Map a [`portable_pty::ExitStatus`] to either an
/// [`ExitCode`] (clean exit) or [`Error::Signal`] (signal death).
///
/// Pure function — no I/O, no globals beyond `libc::strsignal`
/// (which is process-wide but not mutated here).
///
/// # Truncation
///
/// `ExitStatus::exit_code()` is `u32` but `ExitCode` is `u8`
/// on most platforms. We truncate via `& 0xFF`, matching the
/// POSIX `WEXITSTATUS` macro (which discards the upper bits of
/// the wait-status word). Values above 255 are exotic and
/// usually a bug in the child anyway.
pub(crate) fn map_exit_status(status: ExitStatus) -> Result<ExitCode> {
    if let Some(name) = status.signal() {
        let signum = classify_signal_name(name);
        return Err(Error::Signal { signum });
    }
    if status.success() {
        return Ok(ExitCode::SUCCESS);
    }
    Ok(ExitCode::from((status.exit_code() & 0xFF) as u8))
}

/// Try to recover a raw signal number from a
/// `portable_pty::ExitStatus::signal()` string.
///
/// Returns `0` when no layer can classify the input — see
/// the module-level docs for the rationale.
fn classify_signal_name(name: &str) -> i32 {
    if let Some(n) = lookup_via_strsignal(name) {
        return n;
    }
    if let Some(n) = lookup_english_posix(name) {
        return n;
    }
    if let Some(n) = parse_signal_n_literal(name) {
        return n;
    }
    0
}

/// Layer 1 — `#[cfg(unix)]` round-trip through `libc::strsignal`
/// in the current locale.
///
/// Returns `None` on non-Unix targets (where `signal()` is
/// always `None` anyway, so this branch is unreachable in
/// practice) or when no signum in the platform's signal range
/// produces a matching name. The upper bound is `SIGRTMAX()`
/// on platforms that expose it (Linux, Solaris, Hurd) so RT
/// signals are covered, and a generous static fallback (96)
/// elsewhere — comfortably above every Unix signal table I've
/// seen (AIX = 57; macOS / *BSD = 32).
#[cfg(unix)]
fn lookup_via_strsignal(name: &str) -> Option<i32> {
    use std::ffi::CStr;
    let upper = max_signum_probe();
    for n in 1..=upper {
        // SAFETY: `libc::strsignal` returns a pointer into a
        // statically-allocated, NUL-terminated, immutable string
        // owned by libc. We borrow it for the duration of the
        // `CStr::from_ptr` call and immediately copy via
        // `to_string_lossy().into_owned()`, so no aliasing or
        // lifetime issue can leak. A null return is treated as
        // "no name available for this signum" and skipped.
        let rendered = unsafe {
            let ptr = libc::strsignal(n);
            if ptr.is_null() {
                continue;
            }
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        };
        if rendered == name {
            return Some(n);
        }
    }
    None
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "solaris",
    target_os = "illumos",
    target_os = "hurd",
))]
fn max_signum_probe() -> i32 {
    // SIGRTMAX is exposed as a safe `extern fn` (no parameters,
    // no preconditions) on Linux/Solaris/Hurd in `libc`.
    libc::SIGRTMAX()
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "solaris",
    target_os = "illumos",
    target_os = "hurd",
)))]
fn max_signum_probe() -> i32 {
    // Generous static fallback for targets without SIGRTMAX().
    // Comfortably above every Unix signal table observed in
    // practice (AIX = 57; macOS / *BSD = 32).
    96
}

#[cfg(not(unix))]
fn lookup_via_strsignal(_name: &str) -> Option<i32> {
    None
}

/// Layer 2 — small POSIX English-name allowlist for the case
/// where the cached name was produced under a different locale
/// than the current one.
///
/// Uses `libc::SIG*` constants rather than hardcoded Linux
/// signal numbers, so the mapping is correct on macOS and other
/// BSD-family targets where signal numbering diverges (e.g.
/// SIGSYS = 12 on macOS, 31 on Linux). Signals that don't exist
/// on every Unix target are `cfg`-gated. `SIGSTKFLT`, `SIGCLD`,
/// `SIGPOLL`, `SIGUNUSED`, etc. are intentionally omitted —
/// they are aliases or non-POSIX, and a downstream caller that
/// cares can extend this table.
#[cfg(unix)]
fn lookup_english_posix(name: &str) -> Option<i32> {
    // strsignal often prints "Hangup", "Interrupt", "Terminated"
    // — full English words rather than the SIG* tokens — so we
    // accept both spellings and a couple of common variants.
    let lowered = name.to_ascii_lowercase();
    let candidates: &[(&[&str], i32)] = &[
        (&["hangup", "sighup"], libc::SIGHUP),
        (&["interrupt", "sigint"], libc::SIGINT),
        (&["quit", "sigquit"], libc::SIGQUIT),
        (&["illegal instruction", "sigill"], libc::SIGILL),
        (
            &["trace/breakpoint trap", "trace trap", "sigtrap"],
            libc::SIGTRAP,
        ),
        (&["aborted", "abort", "sigabrt"], libc::SIGABRT),
        (&["bus error", "sigbus"], libc::SIGBUS),
        (
            &[
                "floating point exception",
                "floating-point exception",
                "sigfpe",
            ],
            libc::SIGFPE,
        ),
        (&["killed", "sigkill"], libc::SIGKILL),
        (&["user defined signal 1", "sigusr1"], libc::SIGUSR1),
        (&["segmentation fault", "sigsegv"], libc::SIGSEGV),
        (&["user defined signal 2", "sigusr2"], libc::SIGUSR2),
        (&["broken pipe", "sigpipe"], libc::SIGPIPE),
        (&["alarm clock", "sigalrm"], libc::SIGALRM),
        (&["terminated", "sigterm"], libc::SIGTERM),
        (&["child exited", "sigchld"], libc::SIGCHLD),
        (&["continued", "sigcont"], libc::SIGCONT),
        (&["stopped (signal)", "sigstop"], libc::SIGSTOP),
        (&["stopped", "sigtstp"], libc::SIGTSTP),
        (&["stopped (tty input)", "sigttin"], libc::SIGTTIN),
        (&["stopped (tty output)", "sigttou"], libc::SIGTTOU),
        (&["urgent i/o condition", "sigurg"], libc::SIGURG),
        (&["cpu time limit exceeded", "sigxcpu"], libc::SIGXCPU),
        (&["file size limit exceeded", "sigxfsz"], libc::SIGXFSZ),
        (&["virtual timer expired", "sigvtalrm"], libc::SIGVTALRM),
        (&["profiling timer expired", "sigprof"], libc::SIGPROF),
        (&["window changed", "sigwinch"], libc::SIGWINCH),
        (&["i/o possible", "sigio"], libc::SIGIO),
        (&["bad system call", "sigsys"], libc::SIGSYS),
    ];
    for (names, sig) in candidates {
        if names.contains(&lowered.as_str()) {
            return Some(*sig);
        }
    }
    // SIGPWR is Linux/Android-specific; gate separately rather
    // than `cfg`-attribute the table entry, which the
    // const-context restrictions of slice literals don't allow.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        if matches!(lowered.as_str(), "power failure" | "sigpwr") {
            return Some(libc::SIGPWR);
        }
    }
    None
}

#[cfg(not(unix))]
fn lookup_english_posix(_name: &str) -> Option<i32> {
    None
}

/// Layer 3 — parse `"Signal N"` (the literal fallback
/// `portable-pty` emits when `strsignal` returns null).
fn parse_signal_n_literal(name: &str) -> Option<i32> {
    let suffix = name.strip_prefix("Signal ")?;
    suffix.trim().parse::<i32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_exit_zero_maps_to_success() {
        let status = ExitStatus::with_exit_code(0);
        let code = map_exit_status(status).expect("clean exit must be Ok");
        // `ExitCode` has no `Eq`, so compare via debug repr.
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
    }

    #[test]
    fn nonzero_exit_passes_through_truncated() {
        let status = ExitStatus::with_exit_code(7);
        let code = map_exit_status(status).expect("nonzero clean exit must be Ok");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(7)));
    }

    #[test]
    fn exit_255_is_boundary() {
        let status = ExitStatus::with_exit_code(255);
        let code = map_exit_status(status).expect("255 must be Ok");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(255)));
    }

    #[test]
    fn exit_256_truncates_to_zero() {
        // Documented behavior: WEXITSTATUS-style truncation.
        let status = ExitStatus::with_exit_code(256);
        let code = map_exit_status(status).expect("256 must be Ok");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(0)));
    }

    #[cfg(unix)]
    #[test]
    fn english_terminated_maps_to_15() {
        let status = ExitStatus::with_signal("Terminated");
        let err = map_exit_status(status).expect_err("signal must be Err");
        assert!(matches!(err, Error::Signal { signum: 15 }));
    }

    #[cfg(unix)]
    #[test]
    fn english_killed_maps_to_9() {
        let status = ExitStatus::with_signal("Killed");
        let err = map_exit_status(status).expect_err("signal must be Err");
        assert!(matches!(err, Error::Signal { signum: 9 }));
    }

    #[cfg(unix)]
    #[test]
    fn english_hangup_maps_to_1() {
        let status = ExitStatus::with_signal("Hangup");
        let err = map_exit_status(status).expect_err("signal must be Err");
        assert!(matches!(err, Error::Signal { signum: 1 }));
    }

    #[cfg(unix)]
    #[test]
    fn english_interrupt_maps_to_2() {
        let status = ExitStatus::with_signal("Interrupt");
        let err = map_exit_status(status).expect_err("signal must be Err");
        assert!(matches!(err, Error::Signal { signum: 2 }));
    }

    #[test]
    fn signal_n_literal_parses() {
        let status = ExitStatus::with_signal("Signal 17");
        let err = map_exit_status(status).expect_err("signal must be Err");
        assert!(matches!(err, Error::Signal { signum: 17 }));
    }

    #[test]
    fn weird_locale_string_falls_through_to_zero() {
        let status = ExitStatus::with_signal("ZZZ totally bogus name ZZZ");
        let err = map_exit_status(status).expect_err("signal must be Err");
        assert!(matches!(err, Error::Signal { signum: 0 }));
    }

    /// Round-trip guard for layer 1: every signum in `1..=31`
    /// whose `strsignal` returns a non-null, non-empty name
    /// must reverse-lookup to itself in the *current* locale.
    /// This pins the production code path against silent
    /// regressions in the strsignal table or future glibc
    /// updates.
    #[cfg(unix)]
    #[test]
    fn strsignal_round_trip_recovers_signum() {
        use std::ffi::CStr;
        for n in 1..=31i32 {
            let name = unsafe {
                // SAFETY: `libc::strsignal(n)` is a thread-safe
                // POSIX inquiry returning either a NULL pointer
                // (handled below) or a pointer to a process- or
                // thread-local static C string that remains
                // valid until the next strsignal call on the
                // same thread. We do not call strsignal again
                // before consuming the pointer, so the returned
                // `*const c_char` is valid for `CStr::from_ptr`,
                // which copies the bytes into an owned String
                // before the borrow ends.
                let ptr = libc::strsignal(n);
                if ptr.is_null() {
                    continue;
                }
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            };
            if name.is_empty() {
                continue;
            }
            let status = ExitStatus::with_signal(&name);
            let err = map_exit_status(status).expect_err("must be Err");
            match err {
                Error::Signal { signum } => assert_eq!(
                    signum, n,
                    "signum {n} round-tripped via strsignal name {name:?} → {signum}"
                ),
                other => panic!("expected Signal, got {other:?}"),
            }
        }
    }
}
