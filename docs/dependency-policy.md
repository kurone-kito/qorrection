# Dependency policy

This document is the source of truth for how `qorrection` manages
its Rust dependencies. The high-level rules also appear in
[`.github/copilot-instructions.md`](../.github/copilot-instructions.md);
this file holds the operational details.

## Core dependencies

The agreed core set, chosen for cross-platform robustness and
ecosystem maturity:

| Crate | Purpose |
| ----- | ------- |
| `portable-pty` | Cross-platform PTY spawning (Unix PTY + Windows ConPTY) |
| `crossterm` | ANSI / TUI primitives, cursor control, raw mode |
| `clap` | CLI argument parsing (derive feature) |
| `anyhow` | Error handling at the binary boundary |
| `thiserror` | Error types at library module boundaries |
| `tracing` + `tracing-subscriber` | Structured logging and diagnostics |

Justify any new dependency in the commit body. Prefer extending
this set before introducing alternatives.

## Versioning

- **`Cargo.toml`** pins major versions (e.g., `clap = "4"`).
- **`Cargo.lock`** is committed (binary crate) and tracks the
  resolved minor / patch.
- MSRV is declared via `package.rust-version` in `Cargo.toml`.
  Bumping the MSRV is its own atomic commit with a brief
  rationale in the body.

## `cargo update` cadence

- Run `cargo update` deliberately, not opportunistically.
- A lockfile bump is its own commit
  (`chore(deps): cargo update`).
- After updating, re-run `cargo deny check`, `cargo clippy`, and
  `cargo test` before committing.

## License & advisory policy

- Allowed licenses are encoded in [`deny.toml`](../deny.toml).
- Run `cargo deny check` locally before opening a PR; CI must
  enforce the same check (planned).
- Any allow-list change requires a comment in `deny.toml`
  explaining why the entry is acceptable for a dual-licensed
  MIT/Apache-2.0 project.
- Yanked crates are denied; security advisories are escalated
  immediately.

## Duplicate versions

`cargo deny` reports duplicate versions as warnings rather than
errors so the project tolerates short-lived ecosystem skew. A
sustained duplication is a signal to either upgrade the lagging
dependency or pin both consumers to the same version.

## Unsafe and `build.rs`

- `unsafe` is forbidden in our own crate by default (see the AI
  instructions). Dependencies that require `unsafe` internally
  are acceptable.
- Avoid dependencies whose `build.rs` shells out to non-portable
  tools unless platform support already covers Linux, macOS, and
  Windows.
