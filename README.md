# 🚑 qorrection

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![CI](https://github.com/kurone-kito/qorrection/actions/workflows/ci.yml/badge.svg)](https://github.com/kurone-kito/qorrection/actions/workflows/ci.yml)
[![Linting](https://github.com/kurone-kito/qorrection/actions/workflows/lint.yml/badge.svg)](https://github.com/kurone-kito/qorrection/actions/workflows/lint.yml)
[![CodeRabbit](https://img.shields.io/badge/review-CodeRabbit-green?logo=coderabbit)](https://www.coderabbit.ai/)

> A PTY wrapper that catches Vim-style quit commands typed into
> *other* CLI tools and answers them with a passing ambulance —
> because experienced engineers reflexively type `:q` everywhere.

`qorrection` (alias **`q9`** — *kyuukyuu*, the ambulance) wraps
an arbitrary interactive command in a pseudo-terminal, watches
what you type, and intercepts Vim-style quit sequences
(`:q`, `:wq`, `:q!`). Instead of being silently ignored by the
wrapped program, those keystrokes trigger a small ASCII-art
ambulance animation sweeping across the screen, in the same
spirit as the classic `sl` command.

## Status

✅ v0.1.0 — The PTY wrapper, trigger detection, ambulance
animation, and signal handling are fully implemented and
cross-platform tested (Linux, macOS, Windows ConPTY). See
[the roadmap](#roadmap) for what's in scope and what comes next.

## Demo

![qorrection demo](docs/demo/qorrection-demo.gif)

## Why

Modern AI coding agents (GitHub Copilot CLI, Claude Code, Codex
CLI, …) have their own slash-command exit (`/exit`, `/quit`),
but Vim muscle memory makes `:q` slip out anyway. `qorrection`
turns that muscle-memory mistake into a small joke moment without
changing the wrapped program at all.

## Install

Download a self-contained binary from the
[GitHub Releases page][releases] — no Rust toolchain required.
Archives are available for:

- Linux x86_64 and aarch64 (statically linked musl)
- macOS x86_64 and Apple Silicon
- Windows x86_64

Extract the archive, then copy `q9` (or `q9.exe`) to a directory
on your `PATH`.

To build from source instead:

```sh
cargo install --git https://github.com/kurone-kito/qorrection --tag v0.1.0
```

`cargo install qorrection` via [crates.io][crates] is coming soon.

[releases]: https://github.com/kurone-kito/qorrection/releases/latest
[crates]: https://crates.io/crates/qorrection

## Usage

```sh
q9 copilot
q9 claude
```

Anything after the wrapped command's name is forwarded verbatim
as child arguments — including flags. For example,
`q9 claude --help` runs `claude --help`; it does not show
qorrection's own help.

### CLI surface

The v0.1 surface is intentionally minimal:

| Invocation               | Behavior                          |
| ------------------------ | --------------------------------- |
| `q9` (no args)           | Show the usage screen             |
| `q9 -h` / `q9 --help`    | Show the usage screen             |
| `q9 -V` / `q9 --version` | Print `qorrection X.Y.Z` and exit |
| `q9 <cmd> [args...]`     | Enter the wrap path (see below)   |

There are no other flags. Any unrecognized leading `-…` token
(including the bare `--` separator) is rejected on stderr as
`<prog>: unknown option: "<token>"` with exit code 2, where
`<prog>` reflects how the binary was invoked (`q9` or
`qorrection`). Trigger behavior, the 418 gag, and the wrapped
program allowlist are all built in — none of them are
configurable in v0.1.
Detailed trigger edge policies, including bracketed paste, live in
[docs/trigger-policy.md](docs/trigger-policy.md).

### Triggers (locked policy)

Trigger interception is active only when the wrapped command is
one of the known AI coding agents: `copilot`, `codex`, `claude`,
`aichat`, `gemini`, `qwen`, `ollama`. Matching is by command
basename, case-insensitive, with `.exe` / `.cmd` / `.bat`
stripped — so `/usr/bin/Claude` and `claude.exe` both arm. For
any other command the wrapper passes keystrokes through
untouched, so editors such as Vim keep owning `:q`.

| Trigger | Gag                                                          |
| ------- | ------------------------------------------------------------ |
| `:q`    | Standard ambulance with a `FI-FO-FI-FO` siren trail          |
| `:wq`   | Larger ambulance carrying `418 I'm an AI agent`              |
| `:q!`   | 9-car parade (we really wanted `9! = 362880`, but settled)   |

## Roadmap

- [x] PTY plumbing with `portable-pty` (Linux, macOS, Windows ConPTY)
- [x] Trigger detection for `:q`, `:wq`, `:q!`
- [x] Ambulance animation renderer (`crossterm`)
- [x] Signal handling: SIGWINCH forwarding, SIGTERM graceful shutdown
- [ ] Release pipeline (GitHub Releases + crates.io)

## Contributing

See [CONTRIBUTING.md](.github/CONTRIBUTING.md) for general policy
and the AI-agent guidelines in
[.github/copilot-instructions.md](.github/copilot-instructions.md).

This repository follows
[Conventional Commits](https://www.conventionalcommits.org/) and
GitHub Flow. All changes reach `main` through pull requests.

## License

Licensed under either of

- Apache License, Version 2.0
  ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license
  ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project, as defined
in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
