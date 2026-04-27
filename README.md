# 🚑 qorrection

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![CI](https://github.com/kurone-kito/qorrection/actions/workflows/ci.yml/badge.svg)](https://github.com/kurone-kito/qorrection/actions/workflows/ci.yml)
[![Linting](https://github.com/kurone-kito/qorrection/actions/workflows/lint.yml/badge.svg)](https://github.com/kurone-kito/qorrection/actions/workflows/lint.yml)
[![CodeRabbit](https://img.shields.io/badge/review-CodeRabbit-green?logo=coderabbit)](https://www.coderabbit.ai/)

> A PTY wrapper that catches Vim-style quit commands typed into
> *other* CLI tools and answers them with a passing ambulance —
> because experienced engineers reflexively type `:q` everywhere.

`qorrection` (alias **`q9`** — *kyuukyuu*, the ambulance) wraps an
arbitrary interactive command in a pseudo-terminal, watches what
you type, and intercepts Vim-style quit sequences (`:q`, `:wq`,
`:q!`). Instead of being silently ignored by the wrapped program,
those keystrokes trigger a small ASCII-art ambulance animation
sweeping across the screen, in the same spirit as the
classic `sl` command.

## Status

🚧 Early scaffolding. No PTY behavior is implemented yet — the
binaries currently print an "implementation pending" notice. See
[the project plan](#roadmap) for what comes next.

## Why

Modern AI coding agents (GitHub Copilot CLI, Claude Code, Codex
CLI, …) have their own slash-command exit (`/exit`, `/quit`),
but Vim muscle memory makes `:q` slip out anyway. `qorrection`
turns that muscle-memory mistake into a small joke moment without
changing the wrapped program at all.

## Install

> Releases will appear once the wrapper ships its first working
> version. Until then, install from source.

```sh
# From source (requires a recent stable Rust toolchain)
cargo install --git https://github.com/kurone-kito/qorrection
```

Planned distribution channels:

- GitHub Releases (self-contained `qorrection` and `q9` binaries
  for Linux, macOS, Windows)
- `cargo install qorrection` (crates.io)
- Homebrew tap (future)

## Usage

```sh
# Wrap any interactive command. Both `qorrection` and `q9`
# refer to the same binary behavior.
q9 copilot
q9 claude
```

Inside the wrapped session, typing `:q`, `:wq`, or `:q!` plays the
ambulance animation. The exact handoff to the wrapped program — for
example, whether and how to leave Vim alone when it legitimately
owns those keystrokes — is part of the open design work tracked in
the [roadmap](#roadmap), not a guarantee of the current scaffold.

### Options (planned)

| Flag                | Behavior                                                            |
| ------------------- | ------------------------------------------------------------------- |
| `--418`             | Reply with `418 I'm AI Agent` instead of the ambulance              |
| `--no-animation`    | Suppress the animation, keep only the message                       |
| `--triggers <list>` | Override the trigger list (default: `:q,:wq,:q!`)                   |

Exact flag names are subject to change before 1.0.

## Roadmap

1. PTY plumbing with `portable-pty` (Linux, macOS, Windows ConPTY)
2. Trigger detection for `:q`, `:wq`, `:q!`
3. Ambulance animation renderer (`crossterm`)
4. `--418` easter egg
5. Configurable trigger list
6. Release pipeline (GitHub Releases + crates.io)

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
