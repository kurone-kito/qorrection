# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/kurone-kito/qorrection/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/kurone-kito/qorrection/releases/tag/v0.1.0
