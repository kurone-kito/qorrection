# Release Process

Maintenance reference for qorrection releases. Covers the routine
procedure for shipping a new version to crates.io and GitHub Releases,
the lessons accumulated during the v0.1.0 → v0.1.1 path, and the
recovery pattern when a publish fails.

Out of scope: CI configuration (`.github/workflows/ci.yml`), branch
protection rules, and the IDD execution loop that authors the in-repo
bump PR (see [`docs/idd-workflow.md`](idd-workflow.md)).

## Routine release procedure

The pipeline lives in [`.github/workflows/release.yml`](../.github/workflows/release.yml).
A release is two GitHub Actions runs: the tag-push run that creates
the GitHub Release, and a `workflow_dispatch` that runs the
`cargo publish` step inside CI. No local `cargo publish` is needed —
the publish itself happens on a GitHub runner using the
`CARGO_REGISTRY_TOKEN` repository secret.

1. **Author and merge the in-repo bump PR.** Touch only:
   - `Cargo.toml`: bump `version = "X.Y.Z"`.
   - `Cargo.lock`: regenerate by running any cargo build/check command
     with the new `Cargo.toml` in place.
   - `CHANGELOG.md`: promote the `## [Unreleased]` body to
     `## [X.Y.Z] - YYYY-MM-DD` (UTC date — see
     [CHANGELOG date convention](#changelog-date-convention) below).
     Add a new empty `## [Unreleased]` at the top and update the
     link-reference footer.
   - `tests/snapshots/usage_layout__*.snap`: refresh the eight snapshot
     files that embed the version string in rendered usage output. Each
     file's diff is a single line (the version string). Use whichever
     update path is convenient — `cargo insta review` / `cargo insta accept`
     when `cargo-insta` is installed, or an in-editor find-and-replace
     across the eight files.

2. **Tag the merge commit.** From a clean `main` at the merge commit,
   create a **signed annotated tag**:

   ```sh
   git tag -s -a vX.Y.Z -m "vX.Y.Z"
   ```

   Drop the explicit `-s` only when your environment already signs by
   default (e.g. `tag.gpgsign = true` is in your global git config).
   Past qorrection tags (e.g. v0.1.0, v0.1.1) are signed; new tags
   should match.

   Signing key setup (GPG, SSH, or otherwise) is a per-developer
   environment concern — see [git's `gpg.format` documentation](https://git-scm.com/docs/git-config#Documentation/git-config.txt-gpgformat)
   for the supported formats and how to wire `user.signingkey` to the
   right key material for each. Never push an unsigned release tag
   (and never opt out via `--no-sign` / dropping `-s`); if signing
   fails, fix the local environment first.

3. **Push the tag.**

   ```sh
   git push origin vX.Y.Z
   ```

   The tag push triggers `release.yml` on the `v*` tag event. The run
   executes `verify-tag` (asserts `Cargo.toml` matches the tag and is
   not `0.0.0`), builds the five platform archives in parallel
   (`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
   `x86_64-apple-darwin`, `aarch64-apple-darwin`,
   `x86_64-pc-windows-msvc`), and runs `Publish GitHub Release`
   (`softprops/action-gh-release`) to create the GitHub Release with
   all five archives attached and generated release notes.

   `Publish to crates.io (manual)` is intentionally skipped here — it
   only runs on `workflow_dispatch`.

4. **Dispatch the crates.io publish.**

   ```sh
   gh workflow run release.yml --ref vX.Y.Z
   ```

   The dispatch run re-executes `verify-tag` and the build matrix,
   then runs `Publish to crates.io (manual)`:
   `cargo publish --dry-run --locked` first, then the real
   `cargo publish --locked` using the `CARGO_REGISTRY_TOKEN`
   repository secret (only the real publish step receives the
   token — the dry run does not need it). The `--locked` flag
   matches the `release.yml` step definitions exactly and ensures
   the publish uses the committed `Cargo.lock`.

   The dispatch run's `Publish GitHub Release` job **is expected to
   fail** with `Cannot delete asset from an immutable release`. The
   Release was already created by step 3 and is immutable; the dispatch
   re-tries the same step against the same tag. Ignore that failure —
   the job we cared about (`Publish to crates.io (manual)`) is the one
   that determines whether the publish succeeded.

5. **Verify the publish.**

   ```sh
   curl -A "qorrection-release-verify (you@example.com)" \
     https://crates.io/api/v1/crates/qorrection \
     | jq '{max_stable_version: .crate.max_stable_version, versions: [.versions[] | {num, created_at, yanked}]}'
   ```

   crates.io requires a contactable `User-Agent` per its
   [data access policy](https://crates.io/data-access). The default
   `curl/...` string is rejected with HTTP 403 and a
   data-access-policy message; substitute your own maintainer email
   (or the project's published contact) in the `-A` header. The
   response should report `max_stable_version: "X.Y.Z"` and the
   matching version entry with `yanked: false`.

## Lessons from v0.1.0 (postmortem)

The v0.1.0 GitHub Release shipped on 2026-05-17 but never reached
crates.io until v0.1.1 was cut. Three independent GitHub-side
behaviors compounded into a publish that could not be performed
retroactively:

- **GitHub Release immutability.** Releases on this repository are
  created as immutable — `gh release view vX.Y.Z --json isImmutable`
  reports `true` for both v0.1.0 and v0.1.1.
  `softprops/action-gh-release` cannot replace or remove assets on an
  immutable Release; subsequent workflow runs that target the same
  tag fail with
  `Validation Failed: Cannot delete asset from an immutable release`.
  Once the tag-push run creates the Release, that tag's assets are
  frozen.

- **Workflow-file pinning on tag refs.** `gh workflow run release.yml
  --ref vX.Y.Z` runs `release.yml` **as it was at that tag**, not as
  it is on `main`. Jobs added to `release.yml` after the tag was cut
  do not exist on a dispatch from that tag. v0.1.0 hit this because
  the `publish-crates-io` job was added by commit `dd37f5e` after
  v0.1.0 had already been tagged.

- **Transitive `if:` skip propagation.** Before
  [#181](https://github.com/kurone-kito/qorrection/issues/181) /
  [PR #182](https://github.com/kurone-kito/qorrection/pull/182),
  `publish-crates-io` declared only
  `if: github.event_name == 'workflow_dispatch'`. When `verify-tag`
  was skipped on a non-tag dispatch (e.g. from `main`), GitHub
  Actions propagated that skip transitively through the implicit
  `success()` gate, silently skipping `publish-crates-io` as well —
  even though its only direct `needs:` (`build`) had run via its own
  `always()` guard. The current condition includes
  `always() && needs.build.result == 'success'` to survive this.

## Recovery pattern

When a version did not reach crates.io (publish failed, was skipped,
or was never triggered), **bump to the next patch and ship through the
routine procedure**:

- Open the next `X.Y.(Z+1)` bump PR. Add a brief `### Notes`
  subsection under that version's `CHANGELOG.md` entry explaining the
  gap (one paragraph; reference any failed runs by URL).
- Tag and ship the new version per the routine procedure above.

Do not try to retroactively republish under the same version: the
immutability + workflow-pinning combination above makes that path
fragile (and on a tag that predates the `publish-crates-io` job, it is
not possible at all). The v0.1.0 → v0.1.1 recovery is the worked
example —
see [roadmap #177](https://github.com/kurone-kito/qorrection/issues/177)
for the audit trail.

Note: `cargo yank` is a different recovery tool. Yank addresses *post-publish*
corruption (a broken version was successfully published and should no
longer be selected by `cargo add`). This section addresses *failure to
publish at all*; yank does not apply.

## CHANGELOG date convention

`CHANGELOG.md` section headers use the form `## [X.Y.Z] - YYYY-MM-DD`
where the date is **UTC**, matching the tag's GitHub-side
`created_at` (also UTC).

Authors operating in non-UTC timezones should convert before
authoring the header. If the tag will be pushed near midnight UTC,
prefer the UTC date that the tag's `created_at` will actually carry —
when in doubt, push the tag first and read the date back from
`gh release view vX.Y.Z --json createdAt`.

This convention was settled during the PR #179 review pass — CodeRabbit
flagged the original JST stamp (`2026-05-20`) as one day ahead of UTC
(`2026-05-19`) at the time of authoring, and we standardized on UTC to
match the canonical GitHub timestamp.

## Cross-references

- Workflow: [`.github/workflows/release.yml`](../.github/workflows/release.yml)
- Version history: [`CHANGELOG.md`](../CHANGELOG.md)
- Predecessor audit trail:
  [#177](https://github.com/kurone-kito/qorrection/issues/177) (v0.1.1
  release roadmap),
  [#180](https://github.com/kurone-kito/qorrection/issues/180) (workflow
  hardening roadmap),
  [#183](https://github.com/kurone-kito/qorrection/issues/183) (this
  doc's roadmap)
- IDD execution loop that authors the in-repo bump PR:
  [`docs/idd-workflow.md`](idd-workflow.md)
