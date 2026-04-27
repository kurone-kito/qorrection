# AI tooling strategy

This repository implements **qorrection** in Rust. AI-assisted
contributions are first-class because the project began as a vibe-
coded experiment, and Copilot CLI plus VS Code Copilot Chat remain
the primary day-to-day tools.

## Canonical guidance

- [.github/copilot-instructions.md](../.github/copilot-instructions.md)
  is the canonical, fully detailed AI guide. Keep it complete enough
  for GitHub Copilot CLI and VS Code Copilot Chat.
- [AGENTS.md](../AGENTS.md) is a Codex compatibility entry point. It
  must stay self-contained for the rules that Codex needs immediately,
  then point to the canonical Copilot guide for the remaining detail.
- [CLAUDE.md](../CLAUDE.md) is a Claude Code compatibility entry point
  with the same role.
- [GEMINI.md](../GEMINI.md) is a Gemini CLI compatibility entry point
  with the same role.

## Change policy

- Prefer preserving existing Copilot behavior over abstracting too
  early.
- Duplicate only the minimum guidance needed for non-Copilot agents to
  act safely and predictably.
- Extract shared text into a neutral document only after benchmarks
  show that the Copilot-first workflow does not regress.
- When a rule uses a Copilot-specific feature name, document the
  underlying intent so other agents can map it to their own interaction
  model.

## Onboarding detection

Onboarding from the upstream generic template is complete. The
sentinel phrase has been removed from all AI instruction files,
so derived-template detection no longer applies here. If a future
change introduces a second language or otherwise triggers a
template-style restructuring, surface it as a plan-mode proposal
before editing instructions.

## Maintenance notes

- Treat this file as a human-facing strategy note, not as the primary
  instruction file for any agent.
- When updating AI guidance, review `README.md`,
  `.github/copilot-instructions.md`, `AGENTS.md`, `CLAUDE.md`, and
  `GEMINI.md` together.
