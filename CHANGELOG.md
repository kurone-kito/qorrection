# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-05-20

### Added

- **Oversized cameo track** for `:wq` on wide terminals (≥ 160 columns).
  Inspired by `sl`/`nyancat` in scale while keeping repo-owned, pure-ASCII,
  embedded art.
  - Scene contract in `docs/anim-large-art-contract.md` defines eligibility
    (≥ 160 cols), trigger mapping (`:wq` only), frame budget (≤ 60 frames), and
    clipping policy (no horizontal or vertical clipping allowed).
  - Original oversized ASCII locomotive asset: `src/anim/assets/oversized.txt`
    (10 rows × 99 cols, exposed as `car::OVERSIZED`).
  - `WidthBucket::Oversized` width bucket for ≥ 160-column terminals.
  - `:wq` at ≥ 160 cols renders the oversized locomotive sweep (≤ 60 frames at
    50 ms each); `:q` and `:q!` keep their existing large-bucket scenes.

### Notes

- This is the first release published to [crates.io](https://crates.io/crates/qorrection).
  v0.1.0 shipped a GitHub Release on 2026-05-17 but never reached crates.io: the
  `publish-crates-io` workflow job was added after the v0.1.0 tag was cut, and the
  v0.1.0 GitHub Release is immutable, so a retroactive publish through the
  tag-dispatch path was not possible. Bumping to v0.1.1 lets the current workflow
  run end-to-end against a fresh, mutable release.

## [0.1.0] - 2026-05-17

### Added

- PTY wrapper that intercepts Vim-style quit commands (`:q`, `:wq`, `:q!`) typed
  into any CLI tool.
- ASCII ambulance animation plays across the screen when `:q` or `:q!` is typed.
- `418 I'm AI Agent` label annotation on the `:wq` variant.
- `q9` binary alias — *kyuukyuu* (救急), Japanese for ambulance.
- Non-TTY passthrough: when stdin or stdout is not a terminal, the child process
  runs unmodified without any animation.
- Terminal resize forwarding: SIGWINCH on Unix; 250 ms poll-based detection on
  Windows (ConPTY).
- Graceful SIGTERM shutdown that finishes any in-flight animation before exiting.
- Cross-platform support: Linux, macOS, and Windows (ConPTY).

[Unreleased]: https://github.com/kurone-kito/qorrection/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/kurone-kito/qorrection/releases/tag/v0.1.1
[0.1.0]: https://github.com/kurone-kito/qorrection/releases/tag/v0.1.0
