# Dependency policy

This document is the source of truth for how `qorrection` manages
its Rust dependencies. The high-level rules also appear in
[`.github/copilot-instructions.md`](../.github/copilot-instructions.md);
this file holds the operational details.

## Core dependencies

The agreed core set, chosen for cross-platform robustness and
ecosystem maturity. Each entry is added in the same commit that
first depends on it; `Cargo.toml`, this table, and any consuming
code land together.

| Crate | Purpose | Status |
| ----- | ------- | ------ |
| `thiserror` | Crate-level `Error` enum at the library boundary | added |
| `crossterm` | ANSI / TUI primitives, cursor control, raw mode, terminal size | added |
| `libc` (Unix only) | `sigaction`, `pipe2`, `tcsetattr` for the SIGWINCH/SIGTERM self-pipe and raw-mode guard. Gated to `[target.'cfg(unix)'.dependencies]` so Windows builds skip it entirely. | added |
| `portable-pty` | Cross-platform PTY spawning (Unix PTY + Windows ConPTY) | added |
| `anyhow` | Carry `portable-pty`'s `anyhow::Error` results without losing the source chain at the crate `Error` boundary | added |
| `tracing` + `tracing-subscriber` | Optional structured diagnostics, gated by `QORRECTION_LOG` | added |

`clap` is intentionally **not** in this set: the entire CLI
surface is four cases (no args, `-h/--help`, `-V/--version`,
`<cmd> [args...]`) and is hand-rolled in `src/cli/`. This keeps
the dependency footprint small and avoids `clap`'s "smart"
behavior around unknown flags / subcommands fighting our
"first positional that doesn't start with `-` is the wrapped
command" rule.

Justify any new dependency in the commit body. Prefer extending
this set before introducing alternatives.

### Roadmap deps-bundle exception

A pre-approved exception to the "added in the same commit that
first depends on it" rule applies to **roadmap deps-bundle PRs**
— PRs whose explicit purpose is to land a coherent batch of
dependencies for a roadmap phase ahead of the consuming code.
This trades strict per-commit lockstep for the ability to run
the audit gate (MSRV `--locked` check + `cargo deny check`) on
the whole batch in a single PR, and lets downstream feature PRs
focus on behavior rather than dep churn.

When this exception is used:

- the PR description must enumerate the closed dep issues,
- each dep still gets its own atomic commit with rationale,
- the `Status` column in the tables above is flipped to
  `added` only when the dep actually lands in `Cargo.toml`.

## Test-only dependencies

`dev-dependencies` follow the same rule (added in lockstep with
the first commit that uses them, subject to the same roadmap
deps-bundle exception above). The agreed test set:

| Crate | Purpose | Status |
| ----- | ------- | ------ |
| `assert_cmd` | Process-boundary assertions for `tests/*.rs` integration tests | added |
| `predicates` | Companion matcher library used by `assert_cmd` for stdout/stderr/exit-code assertions | added |
| `insta` | ANSI byte-stream snapshot tests for animations and the usage screen | added |
| `rexpect` (Unix only) | Real-PTY end-to-end tests, gated to `[target.'cfg(unix)'.dev-dependencies]` so Windows builds don't pull it transitively | added (`=0.6.3`, MSRV ceiling — see Cargo.toml comment) |
| `tempfile` | Scratch directories for tests that need a real filesystem | added |

## `QORRECTION_LOG` diagnostics policy

`tracing` is wired through a subscriber installed only when the
`QORRECTION_LOG` environment variable is set. The variable
takes a [`tracing-subscriber` `EnvFilter`][envfilter] expression
(e.g. `info`, `qorrection=debug`).

[envfilter]: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html

- **Unset** → no subscriber installed; the wrapper is silent.
- **Set but invalid** → silently treated as "diagnostics off"
  to avoid printing parser noise into the user's interactive
  terminal session.
- **Set and valid** → events are written to **stderr** with the
  parsed filter applied.

The variable is intentionally **`QORRECTION_LOG`**, not
`RUST_LOG`. We do not piggy-back on `RUST_LOG` because the
wrapped child process may itself read `RUST_LOG` and we must
not perturb its environment.

What is **never** logged, regardless of filter level:

- bytes the user types on stdin,
- bytes the wrapped child writes to its stdout / stderr.

Diagnostics are limited to wrapper-internal events (PTY spawn,
signal forwarding, trigger detection state). Anything that
could leak terminal contents stays out of the trace stream.

## Versioning

- **`Cargo.toml`** pins major versions (e.g., `thiserror = "1"`).
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
- CI runs `cargo deny check` on every pull request and on every
  branch push other than `main` (direct pushes to `main` are
  blocked by branch protection, so the gate runs before the
  change can land). Run the same command locally before opening
  a PR.
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
