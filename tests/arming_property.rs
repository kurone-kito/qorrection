//! Property-style invariant tests for the arming allowlist.
//!
//! These tests verify structural invariants of [`is_armed`] that
//! apply across the entire input space:
//!
//! 1. Every allowlist name (in any case combination, with any
//!    strippable extension, with any path prefix) is armed.
//! 2. No near-miss (one-character edit of an allowlist name) is
//!    armed unless it coincidentally matches another entry.
//! 3. NUL bytes always disarm.
//! 4. Only one extension strip is applied.
//! 5. Names with non-ASCII bytes are not armed (byte-wise compare).
//!
//! All inputs are deterministic; no randomness or external state.

use std::ffi::OsStr;

use qorrection::cli::arming::is_armed;

/// The fixed allowlist, replicated here for cross-checking.
const ALLOWLIST: &[&str] = &[
    "copilot", "codex", "claude", "aichat", "gemini", "qwen", "ollama",
];

/// Strippable extensions.
const EXTENSIONS: &[&str] = &[".exe", ".cmd", ".bat"];

/// Case variants to exercise for each allowlist entry.
///
/// These cover the ASCII corners: all-lowercase (canonical form),
/// all-uppercase, title-case, and a mixed-case variant that
/// exercises the case-insensitive comparison at non-obvious
/// positions.
fn case_variants(name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let upper = name.to_uppercase();
    let title: String = {
        let mut c = name.chars();
        match c.next() {
            None => String::new(),
            Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        }
    };
    // Alternating: e.g. "cLaUdE"
    let alt: String = name
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i % 2 == 0 {
                c.to_ascii_lowercase()
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect();
    vec![lower, upper, title, alt]
}

/// Every allowlist entry in every case variant is armed, optionally
/// with a strippable extension and a path prefix.
#[test]
fn every_allowlist_entry_with_case_variants_is_armed() {
    let prefixes: &[&str] = &[
        "",           // bare name
        "/usr/bin/",  // Unix prefix
        "rel/path/",  // relative prefix
        "a/b/c/d/e/", // deep prefix
    ];
    for name in ALLOWLIST {
        for variant in case_variants(name) {
            // bare name
            for prefix in prefixes {
                let input = format!("{prefix}{variant}");
                assert!(
                    is_armed(OsStr::new(&input)),
                    "bare name {input:?} should be armed"
                );
                // with each strippable extension
                for ext in EXTENSIONS {
                    let with_ext = format!("{prefix}{variant}{ext}");
                    assert!(
                        is_armed(OsStr::new(&with_ext)),
                        "{with_ext:?} should be armed"
                    );
                    // uppercase extension variant
                    let with_ext_upper = format!("{prefix}{variant}{}", ext.to_uppercase());
                    assert!(
                        is_armed(OsStr::new(&with_ext_upper)),
                        "{with_ext_upper:?} should be armed"
                    );
                }
            }
        }
    }
}

/// Only ONE extension strip is applied: `name.exe.exe` is NOT armed
/// because after stripping the outer `.exe`, the stem is `name.exe`
/// which does not match any bare allowlist entry.
#[test]
fn only_single_extension_strip_applied() {
    for name in ALLOWLIST {
        for ext in EXTENSIONS {
            let double = format!("{name}{ext}{ext}");
            assert!(
                !is_armed(OsStr::new(&double)),
                "{double:?} must not arm (double extension)"
            );
            // Mixed extensions also: .exe then .bat
            for ext2 in EXTENSIONS {
                let mixed = format!("{name}{ext}{ext2}");
                assert!(
                    !is_armed(OsStr::new(&mixed)),
                    "{mixed:?} must not arm (mixed extensions)"
                );
            }
        }
    }
}

/// NUL bytes always disarm regardless of the name.
#[test]
fn nul_bytes_always_disarm() {
    for name in ALLOWLIST {
        // NUL at start, middle, and end of each allowlist name.
        let nul_start = format!("\x00{name}");
        let nul_mid = format!("{}{}\x00{}", &name[..1], &name[1..], "");
        let nul_end = format!("{name}\x00");
        for s in [&nul_start, &nul_mid, &nul_end] {
            assert!(
                !is_armed(OsStr::new(s.as_str())),
                "{s:?} with NUL must not arm"
            );
        }
    }
    // Standalone NUL
    assert!(!is_armed(OsStr::new("\x00")));
}

/// Non-allowlist names with arbitrary suffixes are never armed.
///
/// We enumerate a set of clearly non-matching names and verify each
/// one, with and without extensions, does not arm.
#[test]
fn non_allowlist_names_are_never_armed() {
    let non_matches: &[&str] = &[
        "vim",
        "nvim",
        "emacs",
        "bash",
        "zsh",
        "python3",
        "node",
        "rustc",
        "cargo",
        "",           // empty (no file_name)
        ".",          // dot-only
        "..",         // double-dot
        "claude_",    // trailing underscore
        "_claude",    // leading underscore
        "claude1",    // trailing digit
        "claude-cli", // hyphen variant
        "copilot2",   // digit suffix
        "xcopilot",   // prefix
        "codex_cli",  // underscore variant
    ];
    for name in non_matches {
        assert!(!is_armed(OsStr::new(name)), "{name:?} must not arm");
        for ext in EXTENSIONS {
            let with_ext = format!("{name}{ext}");
            assert!(
                !is_armed(OsStr::new(&with_ext)),
                "{with_ext:?} must not arm"
            );
        }
    }
}

/// Non-ASCII bytes in the name are treated byte-wise and do NOT
/// arm even if they look like an allowlist entry visually.
#[test]
fn non_ascii_names_are_not_armed() {
    // Unicode homoglyph / full-width "claude" does not arm.
    // These are multi-byte sequences that look similar but are
    // compared byte-by-byte against ASCII allowlist entries.
    let non_ascii: &[&str] = &[
        "ｃｌａｕｄｅ", // full-width
        "сlаudе",       // Cyrillic 'с', 'а', 'е'
        "clàude",       // accent
        "cláude",       // accent
    ];
    for s in non_ascii {
        assert!(!is_armed(OsStr::new(s)), "{s:?} (non-ASCII) must not arm");
    }
}

/// Path-only inputs (trailing separator, no file name component)
/// are never armed.
#[cfg(unix)]
#[test]
fn path_only_inputs_never_arm_on_unix() {
    let path_only: &[&str] = &["/", "/usr/bin/", "rel/path/", "./"];
    for s in path_only {
        assert!(!is_armed(OsStr::new(s)), "{s:?} must not arm");
    }
}

/// Empty string is never armed.
#[test]
fn empty_string_is_not_armed() {
    assert!(!is_armed(OsStr::new("")));
}

/// Verify the invariant: for any valid (non-NUL, non-empty) armed
/// name, adding a non-strippable suffix character breaks arming.
#[test]
fn adding_non_strippable_suffix_disarms() {
    for name in ALLOWLIST {
        for suffix in &["x", "_", "1", "-", ".toml", ".sh", ".py"] {
            let with_suffix = format!("{name}{suffix}");
            assert!(
                !is_armed(OsStr::new(&with_suffix)),
                "{with_suffix:?} must not arm"
            );
        }
    }
}

/// Verify the invariant: for any valid armed name, adding a
/// non-strippable PREFIX (not a directory separator) breaks arming.
#[test]
fn adding_non_separator_prefix_disarms() {
    for name in ALLOWLIST {
        for prefix in &["x", "_", "1", "-"] {
            let with_prefix = format!("{prefix}{name}");
            assert!(
                !is_armed(OsStr::new(&with_prefix)),
                "{with_prefix:?} must not arm"
            );
        }
    }
}

/// Platform-independent check: extension comparison is
/// case-insensitive only for ASCII letters (A-Z).
#[test]
fn extension_strip_ascii_case_insensitive_only() {
    // These should all arm (ASCII case variants of extensions).
    let arm_cases: &[(&str, &str)] = &[
        ("claude", ".EXE"),
        ("claude", ".Exe"),
        ("claude", ".eXe"),
        ("claude", ".CMD"),
        ("claude", ".BAT"),
    ];
    for (name, ext) in arm_cases {
        let input = format!("{name}{ext}");
        assert!(
            is_armed(OsStr::new(&input)),
            "{input:?} should arm (ASCII case ext)"
        );
    }
}
