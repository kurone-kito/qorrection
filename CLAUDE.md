# Guidelines for AI Agents

This repository implements **qorrection** — a PTY wrapper that
intercepts Vim-style quit commands typed into other CLI tools
(`:q`, `:wq`, `:q!`) and replies with playful animations
(ambulance for `:q`-family, optional `418 I'm AI Agent`). The
binary also ships under the alias **`q9`**.

The implementation language is **Rust**, distributed as a single
self-contained binary for Linux, macOS, and Windows.

The repository is currently optimized for GitHub Copilot tooling,
but `CLAUDE.md` exists so Claude Code can still receive the
minimum project rules immediately, without depending on a
redirect.

## Immediate rules

- Match the conversational language to the user's language.
- Write comments and documentation in English unless there is a clear
  project-specific reason otherwise.
- If uncertainty, hidden risk, or missing context blocks a safe change,
  stop and ask a concise question before proceeding.
- Keep changes small and reviewable. If you create commits, follow the
  project's Conventional Commits rules and keep each commit atomic.
- Do not modify community documents (`CODE_OF_CONDUCT*`,
  `CONTRIBUTING*`) without explicit approval.

## Project standards

- **Indentation**: 2 spaces project-wide. Rust source (`*.rs`)
  uses 4 spaces via `rustfmt`; both rules are encoded in
  `.editorconfig`.
- **Line endings**: LF only
- **Trailing whitespace**: trimmed except in Markdown
- **Final newline**: always present
- **File naming**: lowercase with hyphens unless a platform convention
  requires otherwise

## Rust quick rules

- Toolchain pinned to stable via `rust-toolchain.toml`; MSRV in
  `Cargo.toml`.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  and `cargo test` must pass before opening a PR.
- Prefer `anyhow::Result` at the binary boundary; use
  `thiserror`-derived errors at library module boundaries.
- Core dependencies: `portable-pty`, `crossterm`, `clap`,
  `anyhow`, `thiserror`, `tracing`. Justify new ones in the
  commit body.
- `unsafe` is forbidden by default; introducing it requires a
  SAFETY comment and a rubber-duck pass.
- `Cargo.lock` is committed.

## Self-review cycle (rubber-duck pass)

For non-trivial changes (new features, multi-file edits, bug
fixes, PTY/threading/`unsafe` code, public API or CLI changes,
cross-platform behavior), run a rubber-duck self-review **before
opening the PR** and iterate until no rational findings remain.

Skip for trivial changes: typo fixes, IDE-driven renames,
lockfile-only updates, formatting-only commits.

Procedure:

1. After local checks pass, request a rubber-duck critique with
   the diff and relevant context.
2. **Filter** findings — keep correctness, safety, and
   testability issues; discard style nits and speculative ideas
   that would significantly complicate the change without a
   matching benefit.
3. Address each retained finding in its own atomic commit.
4. Re-run the cycle until the filtered finding count is zero.

The full rationale and Copilot-specific procedure live in
[.github/copilot-instructions.md](.github/copilot-instructions.md#self-review-cycle-rubber-duck-pass).

## Commit rules

This project follows
[Conventional Commits](https://www.conventionalcommits.org/).
A `.gitmessage` template is available at the repository root.
Write user-facing, lowercase subjects, keep them under 72 characters,
and split unrelated changes into separate atomic commits.

## Branch strategy

This project follows GitHub Flow. All changes reach `main` through
pull requests (merge commits only — squash and rebase merge are
disabled). Feature branches are always rebased onto `main`, never
merged. See the full rules in
[.github/copilot-instructions.md](.github/copilot-instructions.md#branch-strategy).

## Canonical reference

The full, Copilot-first project guidance lives in
[.github/copilot-instructions.md](.github/copilot-instructions.md).
When that file uses Copilot-specific workflow names, apply the intent
in Claude Code using its own interaction model rather than following
the product terms literally.
