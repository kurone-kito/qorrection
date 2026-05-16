# Oversized ASCII Cameo Scene Contract

This document pins the post-v0.1 contract for oversized ASCII cameo
scenes inspired by the energy of tools such as `sl` and `nyancat`
without copying third-party art or changing the current ambulance gags
accidentally.

## Status

Oversized scenes are a future enhancement track, not a silent rewrite of
the current animation layer.

- v0.1 behavior stays unchanged: `:q` uses the standard ambulance,
  `:wq` uses the larger `418` ambulance, and `:q!` keeps the parade.
- Future oversized work may only ship behind an explicit integration
  path that names this contract in code review and tests.

## Trigger Mapping

The first oversized cameo path is reserved for `:wq`.

- `:wq` is the only trigger allowed to upgrade into an oversized cameo
  scene without another contract update.
- `:q` must keep the standard ambulance identity.
- `:q!` must keep the parade identity unless a later issue extends this
  contract explicitly.

This keeps the current trigger personalities stable while leaving one
high-signal trigger for a future large-format payoff.

## Eligibility

Oversized scenes are eligible only when the terminal can show the full
art without horizontal or vertical clipping.

- Minimum width threshold: `cols >= 160`.
- The renderer must also verify that the terminal has enough visible
  rows for the full asset plus any fixed top padding used by the scene.
- If either dimension is insufficient, the renderer must fall back to
  the existing ambulance scene for that trigger.

The contract deliberately forbids "best effort" partial rendering for
oversized art. Either the full cameo fits, or the user gets the existing
scene unchanged.

## Replace vs Augment

Oversized art augments the current `:wq` path; it does not replace the
ambulance scenes globally.

- The existing ambulance-based scene remains the canonical fallback and
  compatibility path.
- A future oversized scene may appear only as the large-terminal branch
  of the `:wq` renderer decision tree.
- Future work may choose whether the cameo fully replaces the large
  `:wq` timeline or appends after a short ambulance lead-in, but that
  choice must stay entirely within the `:wq` large-terminal branch and
  must not change `:q` or `:q!`.

## Timing Budget

Oversized scenes must stay deterministic and snapshot-testable.

- Reuse the renderer's fixed frame tick (`50 ms`) rather than
  introducing scene-specific sleep durations.
- Encode emphasis by repeating frames in the pure timeline, not by
  adding ad hoc timers in the renderer.
- Keep the oversized cameo timeline at or below `60` frames total,
  including any hold frames.
- Keep the final visible hold at or below `500 ms` within that frame
  budget.

This keeps the path short enough for tests and avoids turning a joke
into a blocking animation.

## Clipping and Fallback

Oversized scenes must fail closed into the existing art path.

- No horizontal clipping of oversized assets is allowed.
- No vertical clipping of oversized assets is allowed.
- If asset dimensions, terminal dimensions, or row-budget plumbing are
  unknown, the implementation must choose the existing ambulance scene.
- Narrow-terminal text fallback remains unchanged and still owns the
  `< 40` column bucket.

## Asset Policy

Oversized assets must be repository-local, ASCII-only, and safe to
embed in the shipped binary.

- Assets must be original to this repository unless a later issue lands
  a separate, explicit license review path.
- Assets must stay pure ASCII.
- Assets must be embedded in the binary as source-controlled text
  (`include_str!` or equivalent static string data).
- Snapshot tests must assert the dimensions and contract-significant
  labels of any oversized asset before renderer integration lands.

## Implementation Boundary

This contract intentionally separates three future concerns:

- `#117` owns original oversized assets.
- `#118` owns renderer integration.
- Any trigger expansion beyond `:wq` requires another contract update
  before implementation.
