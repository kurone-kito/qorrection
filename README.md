# 🚑 qorrection

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![CI](https://github.com/kurone-kito/qorrection/actions/workflows/ci.yml/badge.svg)](https://github.com/kurone-kito/qorrection/actions/workflows/ci.yml)
[![Linting](https://github.com/kurone-kito/qorrection/actions/workflows/lint.yml/badge.svg)](https://github.com/kurone-kito/qorrection/actions/workflows/lint.yml)
[![CodeRabbit](https://img.shields.io/badge/review-CodeRabbit-green?logo=coderabbit)](https://www.coderabbit.ai/)

> A PTY wrapper that catches Vim-style quit commands typed into
> *other* CLI tools and answers them with a passing ambulance —
> because experienced engineers reflexively type `:q` everywhere.

`qorrection` (alias **`q9`** — *kyuukyuu*, the ambulance) is
designed to wrap an arbitrary interactive command in a
pseudo-terminal, watch what you type, and intercept Vim-style
quit sequences (`:q`, `:wq`, `:q!`). Instead of being silently
ignored by the wrapped program, those keystrokes will trigger a
small ASCII-art ambulance animation sweeping across the screen,
in the same spirit as the classic `sl` command. The wrap itself
is not yet implemented (see [Status](#status)).

## Status

🚧 In progress. The CLI surface, usage screen, and version
output are wired and tested end-to-end, but the PTY wrap itself
still prints `qorrection: PTY wrap pending` (exit code 2)
instead of running the child. Trigger detection lives as a
standalone, unit-tested parser module; it does not yet observe
real input. See [the roadmap](#roadmap) for what comes next.

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
# Once PTY plumbing lands, both `qorrection` and `q9` will wrap
# any interactive command. Today these invocations enter the
# wrap stub and exit with `qorrection: PTY wrap pending`.
q9 copilot
q9 claude
```

Anything after the wrapped command's name is parsed as child
arguments and will be forwarded verbatim — including flags — once
PTY plumbing lands. With that in place, `q9 claude --help` will
run `claude --help`; it will not show qorrection's own help.

### CLI surface

The v0.1 surface is intentionally minimal:

| Invocation               | Behavior                          |
| ------------------------ | --------------------------------- |
| `q9` (no args)           | Show the usage screen             |
| `q9 -h` / `q9 --help`    | Show the usage screen             |
| `q9 -V` / `q9 --version` | Print `qorrection X.Y.Z` and exit |
| `q9 <cmd> [args...]`     | Enter the wrap path (see below)   |

The wrap path is the only entry that is not yet fully wired: it
currently prints `qorrection: PTY wrap pending` on stderr and
exits with code 2 until the PTY plumbing milestone lands.

There are no other flags. Any unrecognized leading `-…` token
(including the bare `--` separator) is rejected on stderr as
`<prog>: unknown option: "<token>"` with exit code 2, where
`<prog>` reflects how the binary was invoked (`q9` or
`qorrection`). Trigger behavior, the 418 gag, and the wrapped
program allowlist are all built in — none of them are
configurable in v0.1.

### Triggers (locked policy)

Once PTY plumbing lands, trigger interception will be active
only when the wrapped command is one of the known AI coding
agents: `copilot`, `codex`, `claude`, `aichat`, `gemini`,
`qwen`, `ollama`. Matching is by command basename,
case-insensitive, with `.exe` / `.cmd` / `.bat` stripped — so
`/usr/bin/Claude` and `claude.exe` both arm. For any other
command the wrapper will pass keystrokes through untouched, so
editors such as Vim keep owning `:q`.

| Trigger | Gag                                                          |
| ------- | ------------------------------------------------------------ |
| `:q`    | Standard ambulance with a `FI-FO-FI-FO` siren trail          |
| `:wq`   | Larger ambulance carrying `418 I'm an AI agent`              |
| `:q!`   | 9-car parade (we really wanted `9! = 362880`, but settled)   |

## Roadmap

1. PTY plumbing with `portable-pty` (Linux, macOS, Windows ConPTY)
2. Trigger detection for `:q`, `:wq`, `:q!` (parser already landed;
   wiring it to the live PTY input stream comes with PTY plumbing)
3. Ambulance animation renderer (`crossterm`)
4. Release pipeline (GitHub Releases + crates.io)

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
