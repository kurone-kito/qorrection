//! Capability detection for the controlling terminal.
//!
//! Pure-function core ([`detect_with`]) so unit tests can drive
//! every code path without touching the real environment, plus a
//! thin wrapper ([`detect`]) that snapshots the live process
//! environment for production use.
//!
//! Detection rules (locked v0.1):
//!
//! - `dumb` → `TERM` is unset, empty, or exactly `"dumb"`.
//! - `color` → `!no_color && !dumb && stdout_is_tty`.
//! - `no_color` → the `NO_COLOR` env var is present and non-empty
//!   (per <https://no-color.org/>).
//! - `ci` → the `CI` env var is present and non-empty (the
//!   convention every major CI provider follows).
//! - `utf8` → POSIX locale precedence -- the first non-empty of
//!   `LC_ALL`, `LC_CTYPE`, `LANG` decides; `utf8` is true iff
//!   that winning value mentions `UTF-8` / `utf8`
//!   (case-insensitive). Lower-priority variables are ignored
//!   exactly as `setlocale(3)` does.

use std::io::IsTerminal;

/// Snapshot of the environment variables we read for detection.
///
/// Keeping this as a plain struct lets tests build any
/// combination without `set_var` / `remove_var` calls, which are
/// process-global and racy under `cargo test`'s default thread
/// pool.
#[derive(Debug, Clone, Default)]
pub struct EnvSnapshot {
    pub term: Option<String>,
    pub no_color: Option<String>,
    pub ci: Option<String>,
    pub lc_all: Option<String>,
    pub lc_ctype: Option<String>,
    pub lang: Option<String>,
}

impl EnvSnapshot {
    /// Read the relevant variables from the live process
    /// environment.
    pub fn from_env() -> Self {
        Self {
            term: std::env::var("TERM").ok(),
            no_color: std::env::var("NO_COLOR").ok(),
            ci: std::env::var("CI").ok(),
            lc_all: std::env::var("LC_ALL").ok(),
            lc_ctype: std::env::var("LC_CTYPE").ok(),
            lang: std::env::var("LANG").ok(),
        }
    }
}

/// Resolved view of the terminal we're attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCaps {
    pub stdin_is_tty: bool,
    pub stdout_is_tty: bool,
    pub utf8: bool,
    pub color: bool,
    pub dumb: bool,
    pub ci: bool,
}

/// Snapshot the live terminal capabilities.
pub fn detect() -> TerminalCaps {
    detect_with(
        &EnvSnapshot::from_env(),
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )
}

/// Pure detection from explicit inputs (used by unit tests).
pub fn detect_with(env: &EnvSnapshot, stdin_is_tty: bool, stdout_is_tty: bool) -> TerminalCaps {
    let dumb = is_dumb(env.term.as_deref());
    let no_color = is_set(env.no_color.as_deref());
    let ci = is_set(env.ci.as_deref());
    let utf8 = is_utf8_locale(env);
    let color = stdout_is_tty && !dumb && !no_color;

    TerminalCaps {
        stdin_is_tty,
        stdout_is_tty,
        utf8,
        color,
        dumb,
        ci,
    }
}

fn is_dumb(term: Option<&str>) -> bool {
    match term {
        None => true,
        Some("") => true,
        Some(s) => s.eq_ignore_ascii_case("dumb"),
    }
}

fn is_set(value: Option<&str>) -> bool {
    matches!(value, Some(v) if !v.is_empty())
}

fn is_utf8_locale(env: &EnvSnapshot) -> bool {
    // POSIX precedence: LC_ALL overrides everything; otherwise
    // LC_CTYPE; otherwise LANG. An empty value is treated as
    // unset, matching glibc's `setlocale(3)` behavior.
    [&env.lc_all, &env.lc_ctype, &env.lang]
        .into_iter()
        .filter_map(Option::as_deref)
        .find(|s| !s.is_empty())
        .map(mentions_utf8)
        .unwrap_or(false)
}

fn mentions_utf8(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("utf-8") || lower.contains("utf8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> EnvSnapshot {
        EnvSnapshot::default()
    }

    #[test]
    fn missing_term_is_dumb() {
        let caps = detect_with(&env(), true, true);
        assert!(caps.dumb);
        assert!(!caps.color);
    }

    #[test]
    fn empty_term_is_dumb() {
        let mut e = env();
        e.term = Some(String::new());
        assert!(detect_with(&e, true, true).dumb);
    }

    #[test]
    fn term_dumb_literal_case_insensitive() {
        for v in ["dumb", "DUMB", "Dumb"] {
            let mut e = env();
            e.term = Some(v.into());
            assert!(detect_with(&e, true, true).dumb, "TERM={v}");
        }
    }

    #[test]
    fn xterm_is_not_dumb() {
        let mut e = env();
        e.term = Some("xterm-256color".into());
        let caps = detect_with(&e, true, true);
        assert!(!caps.dumb);
        assert!(caps.color);
    }

    #[test]
    fn no_color_disables_color_even_on_tty() {
        let mut e = env();
        e.term = Some("xterm-256color".into());
        e.no_color = Some("1".into());
        let caps = detect_with(&e, true, true);
        assert!(!caps.color);
    }

    #[test]
    fn empty_no_color_is_ignored() {
        // Spec from no-color.org: presence of a non-empty value.
        let mut e = env();
        e.term = Some("xterm-256color".into());
        e.no_color = Some(String::new());
        let caps = detect_with(&e, true, true);
        assert!(caps.color);
    }

    #[test]
    fn no_color_off_tty_still_no_color() {
        let mut e = env();
        e.term = Some("xterm-256color".into());
        let caps = detect_with(&e, true, false);
        assert!(!caps.color, "color requires stdout TTY");
    }

    #[test]
    fn ci_set_to_anything_nonempty_is_ci() {
        let mut e = env();
        e.ci = Some("true".into());
        assert!(detect_with(&e, true, true).ci);
        e.ci = Some("1".into());
        assert!(detect_with(&e, true, true).ci);
    }

    #[test]
    fn empty_ci_is_not_ci() {
        let mut e = env();
        e.ci = Some(String::new());
        assert!(!detect_with(&e, true, true).ci);
    }

    #[test]
    fn lc_all_utf8_marks_utf8() {
        let mut e = env();
        e.lc_all = Some("en_US.UTF-8".into());
        assert!(detect_with(&e, true, true).utf8);
    }

    #[test]
    fn lc_ctype_utf8_marks_utf8() {
        let mut e = env();
        e.lc_ctype = Some("ja_JP.utf8".into());
        assert!(detect_with(&e, true, true).utf8);
    }

    #[test]
    fn lang_utf8_marks_utf8() {
        let mut e = env();
        e.lang = Some("C.UTF-8".into());
        assert!(detect_with(&e, true, true).utf8);
    }

    #[test]
    fn lc_all_takes_precedence_via_any_match() {
        // Whichever variable mentions UTF-8 is enough.
        let mut e = env();
        e.lang = Some("POSIX".into());
        e.lc_all = Some("en_US.UTF-8".into());
        assert!(detect_with(&e, true, true).utf8);
    }

    #[test]
    fn no_locale_set_is_not_utf8() {
        assert!(!detect_with(&env(), true, true).utf8);
    }

    #[test]
    fn lc_all_overrides_lower_priority_utf8() {
        // POSIX precedence: an LC_ALL of `C` must mask a
        // UTF-8 LANG / LC_CTYPE -- the previous "any match"
        // semantics incorrectly accepted these.
        let mut e = env();
        e.lc_all = Some("C".into());
        e.lang = Some("en_US.UTF-8".into());
        e.lc_ctype = Some("en_US.UTF-8".into());
        assert!(!detect_with(&e, true, true).utf8);
    }

    #[test]
    fn lc_ctype_overrides_lang_utf8() {
        let mut e = env();
        e.lc_ctype = Some("C".into());
        e.lang = Some("en_US.UTF-8".into());
        assert!(!detect_with(&e, true, true).utf8);
    }

    #[test]
    fn empty_lc_all_falls_through_to_lc_ctype() {
        // glibc treats an empty value as unset; the next
        // non-empty variable in the chain must win.
        let mut e = env();
        e.lc_all = Some(String::new());
        e.lc_ctype = Some("en_US.UTF-8".into());
        assert!(detect_with(&e, true, true).utf8);
    }

    #[test]
    fn pure_ascii_locale_is_not_utf8() {
        let mut e = env();
        e.lang = Some("C".into());
        e.lc_all = Some("POSIX".into());
        assert!(!detect_with(&e, true, true).utf8);
    }

    #[test]
    fn stdin_and_stdout_tty_flags_are_independent() {
        let caps = detect_with(&env(), false, true);
        assert!(!caps.stdin_is_tty);
        assert!(caps.stdout_is_tty);

        let caps = detect_with(&env(), true, false);
        assert!(caps.stdin_is_tty);
        assert!(!caps.stdout_is_tty);
    }
}
