# Testing strategy

`qorrection` is a thin joke wrapper, but its correctness depends
on PTY plumbing that is famously platform-dependent. The test
suite is layered so that each layer can fail loud, fast, and
independently.

## Layers

### 1. Unit tests (`#[cfg(test)] mod tests`)

Live next to the production code and cover pure logic with no
PTY, no terminal, and no I/O:

- Trigger detection (`:q`, `:wq`, `:q!`, future variants)
- Animation frame generation
- CLI flag parsing
- Configuration / option resolution

Run with `cargo test --lib` (unit tests live in the library
crate now that `qorrection` is a `lib + bin` package; the bin
targets are thin wrappers and have no own unit tests).

### 2. Integration tests (`tests/`)

Drive the compiled binaries via [`assert_cmd`] for behavior that
must be observed at the process boundary but does not require a
real PTY:

- `--help` / `--version` output stability
- Exit codes for invalid invocations (unknown flags exit `2`,
  help / no-args exit `0` on stdout per POSIX)
- Non-TTY pass-through behavior (piped stdin, redirected stdout)

Run with `cargo test --tests` (Cargo's `--test <name>` flag takes
a single integration-test target name and does not accept globs,
so `cargo test --test '*'` would not match anything).

### 3. PTY end-to-end tests

Use [`rexpect`] (Unix only in v0.1) to drive the wrapper through
a real pseudo-terminal:

- Wrapping a trivial child (e.g., `cat`) and confirming bytes
  pass through unchanged when no trigger fires.
- Typing `:q`, `:wq`, `:q!` and confirming the animation runs
  while the child sees nothing of those keystrokes and the
  wrapper does not exit.
- Confirming child exit propagates to the wrapper exit code.
- Confirming raw mode is restored on cooperative SIGTERM.
- Confirming SIGWINCH forwarding reaches the child PTY.

**Windows policy (v0.1):** all PTY E2E tests are skipped on
Windows. ConPTY behavior diverges from Unix PTYs in ways that
demand a separate harness (no `rexpect` equivalent that we trust
yet), and the v0.1 surface for Windows is best-effort polling
(see plan §6 D-RESIZE). Snapshot + unit tests still run on
Windows.

Two skip mechanisms are in use; choose the one that fits the
test's compilation constraints:

- `#[cfg_attr(windows, ignore = "reason; tracking issue URL")]` —
  use this when the test body can compile on Windows but should
  not run. Every ignored test must carry the tracking issue URL
  so the skip is auditable. See `tests/pty_smoke.rs` for an
  example (tracking issue [#84](https://github.com/kurone-kito/qorrection/issues/84)).
- `#[cfg(unix)]` — use this when the test body uses a Unix-only
  dev-dependency (e.g. `rexpect`) that cannot compile on Windows.
  The `pty_e2e.rs` module is excluded this way; see its module
  comment for the rationale and tracking reference
  ([#64](https://github.com/kurone-kito/qorrection/issues/64)).

Mark a Unix test `#[ignore]` only if it is truly flaky in CI;
document the reason in a doc comment above the test. Always
include a tracking issue link if the ignore is expected to be
temporary.

### 4. Animation snapshot tests

Use [`insta`] to snapshot the rendered ANSI byte stream of each
animation frame. Snapshots are regenerated with
`cargo insta review` and committed in their own commit
(`test(snapshots): refresh ambulance frames`). This keeps frame
edits reviewable without running a real PTY in CI.

## Coverage measurement

### Tool

Install [`cargo-llvm-cov`] once (requires a stable toolchain ≥ 1.87):

```sh
rustup component add llvm-tools-preview --toolchain stable
cargo +stable install cargo-llvm-cov
```

> **Note**: `rust-toolchain.toml` pins the MSRV toolchain (1.78 at
> time of writing). Run `cargo +stable llvm-cov` to use the stable
> toolchain instead, bypassing the directory override.

### Running coverage

```sh
# Summary table for all source files
cargo +stable llvm-cov --lib

# Summary with missing-line numbers printed at the end
cargo +stable llvm-cov --lib --show-missing-lines

# Restrict to the trigger module only
cargo +stable llvm-cov --lib \
  --ignore-filename-regex='src/(?!trigger)' \
  --show-missing-lines
```

### Known remaining gaps (trigger module)

After the tests added in
[#68](https://github.com/kurone-kito/qorrection/issues/68)
the trigger module reaches ≈ 98% line coverage. The residual
uncovered lines are:

| File | Lines | Reason |
| ---- | ----- | ------ |
| `trigger/input.rs` | 196 | `tracing::warn!` in `InputDetector::write` error path — requires `observe_detected_input` to return `Err`, which does not happen in the current test harness because the callback always succeeds. |
| `trigger/input.rs` | 673–675, 705–707 | Inline `on_trigger` closures in tests that intentionally verify the trigger does **not** fire; the closure body is never invoked. Production trigger-fire closure coverage is provided by other tests. |

## Coverage targets

No hard coverage percentage is enforced — covering every PTY edge
case is impractical. The intent is:

- Trigger detection: ≥ 98% line coverage; 100% line + branch for
  `parser.rs`, `paste.rs`, and `altscreen.rs`; the residual gap in
  `input.rs` is documented in the table above
- Animation rendering: snapshot coverage of every frame
- PTY plumbing: at least one happy-path E2E test per supported
  platform

## CI matrix

The `CI` workflow runs the suite on Linux, macOS, and Windows.
PTY E2E tests run on Linux and macOS; on Windows they are
`#[ignore]` in v0.1 (see PTY layer above). Snapshot and unit
tests must pass on all three platforms.

## Running locally

```sh
cargo test                        # all layers
cargo test --lib                  # unit tests in the library
cargo test --tests                # integration + E2E
cargo insta review                # review pending snapshots
```

## Release build verification

Before every release, trigger the `release.yml` workflow via
`workflow_dispatch` to verify all five cross-compilation targets
build successfully and produce artifacts:

```sh
gh workflow run release.yml --ref main
```

When run without a tag, the `verify-tag` job is skipped and the
`build` matrix still runs, producing archives for all five targets:

| Target                     | Platform      | Archive |
| -------------------------- | ------------- | ------- |
| `x86_64-unknown-linux-musl` | Linux x64    | tar.gz  |
| `aarch64-unknown-linux-musl` | Linux arm64 | tar.gz  |
| `x86_64-apple-darwin`       | macOS x64    | tar.gz  |
| `aarch64-apple-darwin`      | macOS arm64  | tar.gz  |
| `x86_64-pc-windows-msvc`    | Windows x64  | zip     |

Artifacts are uploaded to the workflow run and available for 90 days.
The `publish` job (GitHub Release creation) is gated on the tag path
and does not run on `workflow_dispatch`.

## Adding a test

1. Pick the lowest layer that can express the behavior. Pure
   logic belongs in unit tests; binary-boundary behavior in
   integration tests; terminal-interaction behavior in E2E.
2. If the test needs a snapshot, add it under a dedicated
   `snapshots/` subdirectory next to the test file.
3. New PTY E2E tests need a brief comment naming the platform
   limitations they accept.

[`assert_cmd`]: https://crates.io/crates/assert_cmd
[`cargo-llvm-cov`]: https://crates.io/crates/cargo-llvm-cov
[`rexpect`]: https://crates.io/crates/rexpect
[`insta`]: https://crates.io/crates/insta
