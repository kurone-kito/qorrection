//! Trigger arming policy (allowlist of wrapped commands).
//!
//! Phase 1 deliverable: a single pure function, [`is_armed`],
//! that decides whether the wrapped child command is one of the
//! AI CLIs whose `:q`-style typings should be intercepted by the
//! Phase 2+ pump. Everything else (the actual interception, the
//! animation, the pump wiring) is out of scope for this module.
//!
//! ## Spec lock
//!
//! From the locked v0.1 spec ([meta issue #11]):
//!
//! - The allowlist is **fixed** (not configurable):
//!   `copilot`, `codex`, `claude`, `aichat`, `gemini`, `qwen`,
//!   `ollama`.
//! - Matching uses the **basename** of the wrapped command,
//!   with a single `.exe` / `.cmd` / `.bat` suffix stripped if
//!   present (case-insensitive on the suffix), then compared
//!   ASCII case-insensitively against the allowlist.
//! - Comparison is **byte-wise**: full-width Unicode lookalikes
//!   (`ｃｌａｕｄｅ`) and Cyrillic homoglyphs do **not** arm.
//!
//! [meta issue #11]: https://github.com/kurone-kito/qorrection/issues/11
//!
//! ## Path semantics
//!
//! Basename extraction is delegated to [`std::path::Path::file_name`].
//! That means platform-native separators apply: `/` on every
//! target, plus `\` (and Windows-style drive prefixes like
//! `C:`) on `cfg(windows)`. As a consequence,
//! `C:\\tools\\claude.exe` arms on Windows and does **not** arm
//! on Unix (where the whole string is one filename). This
//! matches how the OS will actually resolve the command and
//! avoids surprising false positives on Unix filenames that
//! legitimately contain a backslash.
//!
//! ## NUL guard
//!
//! Synthetic [`std::ffi::OsStr`] inputs that contain a NUL byte
//! cannot be passed to real `exec`/`CreateProcess` calls but
//! could otherwise sneak past basename extraction. We reject
//! them up front so the function's behaviour matches the
//! "command we would actually try to spawn" intuition.

use std::ffi::OsStr;
use std::path::Path;

/// Suffixes stripped (case-insensitively, one shot) before
/// comparing against [`ALLOWLIST`].
const STRIPPABLE_SUFFIXES: &[&[u8]] = &[b".exe", b".cmd", b".bat"];

/// Fixed allowlist of AI CLIs whose Vim-style quit literals are
/// intercepted by the wrapper. Order is irrelevant; the matcher
/// returns on first hit.
const ALLOWLIST: &[&[u8]] = &[
    b"copilot", b"codex", b"claude", b"aichat", b"gemini", b"qwen", b"ollama",
];

/// Returns `true` iff `command`'s basename (after a single
/// optional `.exe`/`.cmd`/`.bat` strip) ASCII-case-insensitively
/// matches one of the locked allowlist entries.
///
/// Never panics; never allocates. See the [module-level
/// documentation](self) for the spec lock and platform notes.
pub fn is_armed(command: &OsStr) -> bool {
    // NUL guard: real spawn paths reject these, and they would
    // otherwise let synthetic OsStr inputs bypass basename rules.
    if command.as_encoded_bytes().contains(&0) {
        return false;
    }

    let Some(basename) = Path::new(command).file_name() else {
        return false;
    };
    let bytes = basename.as_encoded_bytes();
    let stem = strip_known_suffix(bytes).unwrap_or(bytes);
    if stem.is_empty() {
        return false;
    }
    ALLOWLIST
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(stem))
}

/// If `bytes` ends with one of [`STRIPPABLE_SUFFIXES`]
/// (case-insensitive on ASCII) and the stem before that suffix
/// is non-empty, return the stem. Otherwise return `None` so the
/// caller keeps the original bytes.
///
/// Strip is single-shot: `claude.exe.bak` does not match (the
/// `.bak` is not strippable), and `claude.exe.cmd` keeps only
/// the trailing `.cmd` removed (leaving `claude.exe`, which is
/// not in the allowlist).
fn strip_known_suffix(bytes: &[u8]) -> Option<&[u8]> {
    for suffix in STRIPPABLE_SUFFIXES {
        if bytes.len() > suffix.len() {
            let split_at = bytes.len() - suffix.len();
            let (stem, tail) = bytes.split_at(split_at);
            if tail.eq_ignore_ascii_case(suffix) {
                return Some(stem);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::is_armed;
    use std::ffi::OsStr;

    /// Portable cases that work uniformly on every target.
    /// Backslash-bearing strings are tested under
    /// `cfg(windows)`-only blocks below to avoid baking in
    /// platform-specific path semantics on Unix.
    const PORTABLE_CASES: &[(&str, bool)] = &[
        // bare allowlist entries (each one covered)
        ("copilot", true),
        ("codex", true),
        ("claude", true),
        ("aichat", true),
        ("gemini", true),
        ("qwen", true),
        ("ollama", true),
        // ASCII case folding
        ("Claude", true),
        ("CLAUDE", true),
        ("CoPiLoT", true),
        // single-shot extension strip (case-insensitive on suffix)
        ("claude.exe", true),
        ("CLAUDE.EXE", true),
        ("Claude.Exe", true),
        ("claude.cmd", true),
        ("claude.CMD", true),
        ("claude.bat", true),
        ("Claude.BAT", true),
        // path components -- basename is what counts
        ("/usr/bin/claude", true),
        ("/usr/local/bin/Claude.exe", true),
        ("./claude", true),
        ("bin/claude", true),
        // negatives: not on allowlist
        ("vim", false),
        ("nano", false),
        ("zsh", false),
        // negatives: unknown extension is NOT stripped
        ("claude.bak", false),
        // single-shot strip: only the trailing suffix is removed
        ("claude.exe.bak", false),
        ("claude.exe.cmd", false), // strips .cmd, leaves "claude.exe" which is not allowlisted
        // dot-leading: stem becomes ".claude", not "claude"
        (".claude.exe", false),
        // empty stem after strip
        (".exe", false),
        (".cmd", false),
        (".bat", false),
        // super/substrings of allowlist entries
        ("claudex", false),
        ("clau", false),
        ("xclaude", false),
        // empty / whitespace / separator-only
        ("", false),
        ("   ", false),
        ("/", false),
        ("/usr/bin/", false), // Path::file_name returns None
        // Unicode lookalikes must NOT match (ASCII-only fold)
        ("\u{ff43}\u{ff4c}\u{ff41}\u{ff55}\u{ff44}\u{ff45}", false), // ｃｌａｕｄｅ
        ("\u{0441}laude", false), // Cyrillic small letter es + 'laude'
        ("clauder", false),
    ];

    #[test]
    fn portable_matrix() {
        for (input, expected) in PORTABLE_CASES {
            let actual = is_armed(OsStr::new(input));
            assert_eq!(
                actual, *expected,
                "is_armed({input:?}) returned {actual}, expected {expected}"
            );
        }
    }

    #[test]
    fn allowlist_is_complete() {
        // Hard guard so an accidental allowlist edit cannot pass
        // CI by removing both the entry and its positive case.
        for entry in [
            "copilot", "codex", "claude", "aichat", "gemini", "qwen", "ollama",
        ] {
            assert!(
                is_armed(OsStr::new(entry)),
                "allowlist regression: {entry:?} should arm"
            );
        }
    }

    #[cfg(unix)]
    mod unix {
        use super::is_armed;
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        #[test]
        fn invalid_utf8_with_no_match_is_false() {
            // Non-UTF-8 lead bytes; basename byte-content does
            // not equal any allowlist entry.
            let raw: &[u8] = b"\xff\xfeclaude_no";
            assert!(!is_armed(OsStr::from_bytes(raw)));
        }

        #[test]
        fn invalid_utf8_path_with_matching_basename_is_true() {
            // Directory part has invalid UTF-8 but the basename
            // bytes equal "claude" exactly -- arms.
            let raw: &[u8] = b"\xff\xfe/claude";
            assert!(is_armed(OsStr::from_bytes(raw)));
        }

        #[test]
        fn embedded_nul_is_rejected() {
            let raw: &[u8] = b"clau\0de";
            assert!(!is_armed(OsStr::from_bytes(raw)));
        }

        #[test]
        fn embedded_nul_in_path_part_is_rejected() {
            let raw: &[u8] = b"junk\0/claude";
            assert!(!is_armed(OsStr::from_bytes(raw)));
        }

        #[test]
        fn invalid_utf8_path_with_extension_in_basename_arms() {
            // Non-UTF-8 directory part, basename "claude.EXE" --
            // strip + ASCII fold should still arm.
            let raw: &[u8] = b"\xff\xfe/claude.EXE";
            assert!(is_armed(OsStr::from_bytes(raw)));
        }
    }

    #[cfg(windows)]
    mod windows {
        use super::is_armed;
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        fn wide(s: &str) -> OsString {
            OsString::from_wide(&s.encode_utf16().collect::<Vec<u16>>())
        }

        #[test]
        fn drive_letter_path_arms() {
            assert!(is_armed(&wide(r"C:\tools\claude.exe")));
        }

        #[test]
        fn drive_letter_trailing_separator_does_not_arm() {
            assert!(!is_armed(&wide(r"C:\tools\")));
        }

        #[test]
        fn forward_slash_path_arms_on_windows_too() {
            assert!(is_armed(&wide("C:/tools/claude.exe")));
        }

        #[test]
        fn bare_basename_arms() {
            assert!(is_armed(&wide("claude.exe")));
        }
    }
}
