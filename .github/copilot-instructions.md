# Guidelines for AI Agents

This repository implements **qorrection** — a PTY wrapper that
intercepts Vim-style quit commands typed into other CLI tools
and replies with playful animations (e.g. a passing ambulance
for `:q`, `:wq`, `:q!`). The published binary also ships under
the alias **`q9`** (*kyuukyuu*, the Japanese word for ambulance).

The implementation language is **Rust**, distributed as a single
self-contained binary for Linux, macOS, and Windows. Joke value
notwithstanding, the engineering bar matches `sl`: clean code,
robust cross-platform PTY handling, and meaningful tests.

When contributing to this repository using AI agents, adhere to
the following guidelines to ensure high-quality contributions
that align with the project's standards and practices:

## Tooling priority and compatibility

This repository is intentionally optimized for GitHub Copilot CLI and
VS Code Copilot Chat because they are the primary tools used for
day-to-day work and benchmarking.

`AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` exist as lightweight
compatibility entry points for Codex, Claude Code, and Gemini CLI.
Keep this file as the canonical, fully detailed guide unless benchmark
results justify a more neutral layout.

## Conversation

- The conversational language should match the user's language.
  For example, if the user speaks in Japanese, respond in Japanese.
- However, comments and documentation should be written in English unless
  there is a clear context otherwise.
- If uncertainties, concerns, or other implementation issues arise while
  running in Agent mode, promptly switch to Plan mode and ask the user
  questions. In such cases, provide one or more recommended response
  options.
- Outside GitHub Copilot, interpret the `Agent mode` and `Plan mode`
  wording by intent: continue autonomously for low-risk work, but pause
  and ask a concise question when uncertainty or hidden risk makes the
  next step unsafe. When that pause is needed, provide one or more
  recommended response options.

## Branch strategy

This project follows
[GitHub Flow](https://docs.github.com/en/get-started/using-git/github-flow):
`main` is the only long-lived branch and every change reaches `main`
through a pull request.

### Rules

- **Never push directly to `main`** — all changes must go through a
  pull request. Branch protection is enforced on GitHub.
- **Rebase onto `main`** — when a feature branch needs the latest
  `main`, always rebase (`git pull --rebase` or
  `git rebase main`). Do not create merge commits inside feature
  branches.
- **Rebase between feature branches** — if one feature branch needs
  changes from another, use rebase, not merge.
- **Merge commits at PR boundary** — pull requests into `main` are
  merged with a merge commit (squash-merge and rebase-merge are
  disabled in the repository settings).
- **fixup + autosquash for in-branch fixes** — when a later commit in
  a feature branch fixes an earlier one, prefer
  `git commit --fixup=<sha>` followed by
  `git rebase -i --autosquash` to fold the fix into its target.
- **Avoid giant commits** — if squashing would produce an
  unreasonably large commit, keep the fix commit separate or
  re-split the history so each commit remains reviewable.

## Commit rules

This project follows
[Conventional Commits](https://www.conventionalcommits.org/).
A `.gitmessage` template is available at the repository root for
guidance when writing commit messages.

### Format

```txt
<type>[optional scope]: <user-facing description>

<body: address purpose, context, and what changed>

[optional footer(s)]
```

### Subject line

- Use the format: `<type>[optional scope]: <description>`
- Write from the **user's perspective** — briefly state what this
  commit solves or improves for the end user or developer
- Write in **lowercase**, imperative mood (e.g., "add", not "added")
- Keep the subject line under **72 characters**
- Do **not** end with a period

### Types

Common types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`,
`chore`, `ci`, `build`, `perf`

### Scopes

- Optional, in parentheses: `feat(ci):`, `fix(lint):`, `docs(readme):`
- Keep scopes **lowercase**, short, and consistent
- Use the directory or component name that best describes the area

### Body (line 3+)

The body should address three aspects:

- **Why** — the purpose or motivation behind the change
- **Context** — what was needed, the situation or constraint
- **What changed** — the concrete action taken

Prefer the **why → context → change** order when practical.
Write these as **natural prose** — weave the aspects into
coherent sentences rather than using labeled sections. Labeled
sections (`Why:` / `Context:` / `Change:`) are acceptable only
when explicit paragraph separation improves clarity.

Omit any aspect whose information **cannot be reliably inferred**.
If the subject line is self-explanatory, the body may be omitted
entirely. **Breaking changes must always include a body.**

Wrap body lines at **72 characters**.

### Breaking changes

- Append `!` after the type/scope: `feat!: remove deprecated endpoint`
- Add a `BREAKING CHANGE:` trailer in the footer with a detailed
  explanation of what breaks and migration steps

### Footers / trailers

- `Closes #<issue>` / `Refs #<issue>` — link to issues
- `Co-authored-by: Name <email>` — credit co-authors
- `BREAKING CHANGE: <description>` — detail the breaking change

### Atomic commits

Keep each commit as **small and focused** as possible:

- **One logical change per commit** — if the subject line needs "and",
  consider splitting
- **Separate refactoring** from behavior changes
- **Separate formatting/style** changes from logic changes
- **Separate dependency updates** from code changes
- When in doubt, prefer smaller commits that are easy to review,
  revert, and bisect

### Examples

#### Good — single-line (trivial change)

```txt
fix: correct typo in feature request template
```

#### Good — prose body

```txt
feat(ci): add concurrency settings to lint workflow

Parallel lint runs on the same branch waste resources and
cause race conditions in status checks. GitHub Actions
supports concurrency groups that automatically cancel
redundant runs, so add a concurrency group keyed on branch
name with cancel-in-progress enabled.

Refs #42
```

#### Good — breaking change

```txt
feat!: require node 20 as minimum version

Node 18 reaches end-of-life and lacks native fetch support
used by the new HTTP client. All production environments
have already been upgraded to node 20+, so update the
engines field and CI matrix to require node >= 20.

BREAKING CHANGE: drop support for node 16 and 18. Users
must upgrade to node 20 or later.
Closes #108
```

#### Bad — vague, developer-centric

```txt
fix: update code
```

#### Bad — too large / non-atomic

```txt
feat: add auth system and refactor database layer and update docs
```

## Coding Standards

- **Indentation**: 2 spaces project-wide (enforced by
  `.editorconfig`). **Exception:** Rust source files (`*.rs`)
  use 4-space indentation as enforced by `rustfmt`; the
  `.editorconfig` already encodes this override.
- **Line endings**: LF only (enforced by `.editorconfig` and
  `.gitattributes`)
- **Trailing whitespace**: trimmed (except in Markdown)
- **Final newline**: always present
- **File naming**: lowercase with hyphens (e.g., `feature-request.yml`)
  unless constrained by a platform convention (e.g., `CONTRIBUTING.md`)

## Rust conventions

- **Toolchain**: `stable`, pinned via `rust-toolchain.toml`. MSRV
  is declared in `Cargo.toml` (`rust-version`); bumping the MSRV
  requires its own atomic commit.
- **Formatting**: `cargo fmt --check` must pass. `rustfmt.toml`
  controls the rules — do not opt out per-file.
- **Lints**: `cargo clippy --all-targets -- -D warnings` must pass.
  Allow only with a written justification, scoped narrowly
  (`#[allow(clippy::<lint>)] // <why>`).
- **Error handling**: `anyhow::Result` at the binary boundary;
  define explicit `thiserror`-derived errors at library module
  boundaries when one is introduced.
- **Logging / diagnostics**: prefer `tracing` over `eprintln!`
  once the project grows beyond placeholder code.
- **Dependencies**: justify each new dependency in the commit
  body. Prefer the agreed core set (`portable-pty`, `crossterm`,
  `clap`, `anyhow`, `thiserror`, `tracing`) before introducing
  alternatives.
- **`unsafe`**: forbidden by default. Any introduction requires
  a SAFETY comment and a rubber-duck review pass.
- **Cargo.lock**: committed (this crate is a binary).

## Testing strategy

- **Unit tests** live next to the code (`#[cfg(test)] mod tests`).
- **Integration tests** live under `tests/` and exercise the
  binaries through `assert_cmd`.
- **PTY end-to-end tests** drive the wrapper through a real PTY
  with `rexpect` (or equivalent). Mark them `#[ignore]` if they
  cannot run reliably in CI on a given platform, and document
  why.
- **Animation snapshots** use `insta` so ANSI frame output is
  reviewable without running a real PTY. Snapshot updates
  belong in their own commit.
- **Cross-platform coverage**: CI runs on Linux, macOS, and
  Windows. Windows ConPTY behavior must be exercised early —
  do not assume Unix-only PTY semantics.

## Dependency policy

- Run `cargo update` deliberately, not opportunistically. A
  lockfile bump is its own commit.
- Treat `cargo deny` (license + advisory) as the source of
  truth; any allow-list entry needs a comment.
- Pin major versions in `Cargo.toml`; let `Cargo.lock` track
  the minor/patch.

## Self-review cycle (rubber-duck pass)

For non-trivial changes, run a rubber-duck self-review **before
opening the PR** and iterate until no rational findings remain.

**When required (must run):**

- New features or new modules
- Changes touching multiple files or crossing module boundaries
- Bug fixes whose root cause is non-obvious
- PTY, threading, async, or `unsafe` code
- Changes to public APIs or CLI flags
- Cross-platform behavior changes (anything Windows-specific
  qualifies)

**When optional (skip is fine):**

- Pure typo / wording fixes in docs and comments
- Mechanical renames handled by an IDE refactor
- Dependency lockfile-only updates
- Formatting-only commits (`cargo fmt`)

**Procedure:**

1. After implementation passes local checks (`cargo fmt --check`,
   `cargo clippy -D warnings`, `cargo test`), invoke the
   rubber-duck agent with the relevant diff and context.
2. **Filter the findings** — keep those that prevent bugs,
   correctness issues, race conditions, security holes, or
   clearly improve testability. Discard style nits, speculative
   "could be nicer" suggestions, and findings that would
   significantly complicate the implementation without a
   matching benefit. Briefly note the rationale for each
   discard in the working notes.
3. Address every retained finding in **separate atomic commits**
   (do not amend the original feature commit).
4. Re-run the rubber-duck pass on the updated diff.
5. Repeat until the filtered finding count is zero.

This cycle is intentionally costly. It is the highest-leverage
quality lever available, and applying it only to non-trivial
changes keeps the cost proportional to the risk.

## Guardrails

- **Do not** modify community documents (CODE_OF_CONDUCT, CONTRIBUTING)
  without explicit approval

## Onboarding

This repository has already completed the initial onboarding from
the upstream generic template. Treat the AI guidance, tooling,
and CI as project-specific. If a future change requires another
round of onboarding-style customization (e.g., adopting a new
language alongside Rust), surface it as a plan-mode proposal
before editing instructions or workflows.
