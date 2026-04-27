# Guidelines for AI Agents

This project is a language-independent generic project template.

When contributing to this repository using AI agents, adhere to the
following guidelines to ensure high-quality contributions that align with
the project's standards and practices:

## Conversation

- The conversational language should match the user's language.
  For example, if the user speaks in Japanese, respond in Japanese.
- However, comments and documentation should be written in English unless
  there is a clear context otherwise.
- If uncertainties, concerns, or other implementation issues arise while
  running in Agent mode, promptly switch to Plan mode and ask the user
  questions. In such cases, provide one or more recommended response
  options.

## Commits rules

Try to keep your commits atomic. If you are planning a large commit,
carefully consider whether it can be broken down into smaller parts.

This project follows
[Conventional Commits](https://www.conventionalcommits.org/).

### Rules of the Conventional Commits

- Use the format: `<type>[optional scope]: <description>`
- Common types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`,
  `chore`, `ci`, `build`, `perf`
- Write the subject line in **lowercase**, imperative mood
- Keep the subject line under 72 characters
- Use the body to explain _what_ and _why_, not _how_

Examples:

```text
feat: add issue template chooser config
fix: correct typo in feature request template
docs: expand README with editor support matrix
chore: update cspell word list
ci: add concurrency settings to lint workflow
```

## Coding Standards

- **Indentation**: 2 spaces (enforced by `.editorconfig`)
- **Line endings**: LF only (enforced by `.editorconfig` and
  `.gitattributes`)
- **Trailing whitespace**: trimmed (except in Markdown)
- **Final newline**: always present
- **File naming**: lowercase with hyphens (e.g., `feature-request.yml`)
  unless constrained by a platform convention (e.g., `CONTRIBUTING.md`)

## Guardrails

- **Do not** modify community documents (CODE_OF_CONDUCT, CONTRIBUTING)
  without explicit approval

## Onboarding

This project template is generic and language-independent.
If you plan to implement a language-specific project based on this one,
**submit a proposal to customize this documentation first**.
