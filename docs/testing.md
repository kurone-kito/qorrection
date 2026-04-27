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

Run with `cargo test --lib`.

### 2. Integration tests (`tests/`)

Drive the compiled binaries via [`assert_cmd`] for behavior that
must be observed at the process boundary but does not require a
real PTY:

- `--help` / `--version` output stability
- Exit codes for invalid invocations
- Non-interactive `--418` mode

Run with `cargo test --test '*'`.

### 3. PTY end-to-end tests

Use [`rexpect`] (Unix) and an equivalent ConPTY harness on
Windows to drive the wrapper through a real pseudo-terminal:

- Wrapping a trivial child (e.g., `cat`) and confirming bytes
  pass through unchanged when no trigger fires.
- Typing `:q`, `:wq`, `:q!` and confirming the animation runs
  while the child sees nothing of those keystrokes.
- Confirming child exit propagates to the wrapper exit code.

Mark a test `#[ignore]` if it cannot run reliably on a given
platform in CI; document the reason in a doc comment above the
test. Always include a tracking issue link if the ignore is
expected to be temporary.

### 4. Animation snapshot tests

Use [`insta`] to snapshot the rendered ANSI byte stream of each
animation frame. Snapshots are regenerated with
`cargo insta review` and committed in their own commit
(`test(snapshots): refresh ambulance frames`). This keeps frame
edits reviewable without running a real PTY in CI.

## Coverage targets

No hard coverage percentage is enforced — covering every PTY edge
case is impractical. The intent is:

- Trigger detection: 100% line + branch
- Animation rendering: snapshot coverage of every frame
- PTY plumbing: at least one happy-path E2E test per supported
  platform

## CI matrix

The `CI` workflow runs the suite on Linux, macOS, and Windows.
PTY E2E tests **must** be exercised on all three so Windows
ConPTY regressions surface immediately rather than at release
time.

## Running locally

```sh
cargo test                        # all layers
cargo test --lib                  # unit tests only
cargo test --test '*'             # integration + E2E
cargo insta review                # review pending snapshots
```

## Adding a test

1. Pick the lowest layer that can express the behavior. Pure
   logic belongs in unit tests; binary-boundary behavior in
   integration tests; terminal-interaction behavior in E2E.
2. If the test needs a snapshot, add it under a dedicated
   `snapshots/` subdirectory next to the test file.
3. New PTY E2E tests need a brief comment naming the platform
   limitations they accept.

[`assert_cmd`]: https://crates.io/crates/assert_cmd
[`rexpect`]: https://crates.io/crates/rexpect
[`insta`]: https://crates.io/crates/insta
