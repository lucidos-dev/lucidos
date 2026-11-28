# 0829: A lane batch's /harden counts each member's full marker at the merged snapshot as reviewed, so it reviews and tests only the lane's resolutions

- **Status**: Accepted
- **Date**: 2026-11-27
- **Amends**: [0465](0465-integrated-sub-threads.md),
  [0758](0758-lane-catch-up-certificates.md)

## Context

A lane batch merged one member whose only conflict was `docs/adr/index.md`:
both sides claimed one ADR number. Its `/harden` still printed
`FULL no harden marker`. It ran a high-effort review subagent, a Codex review
and the full suite set over all 29 files of the member's diff.

The member had already passed a full `/harden` at the exact commit the lane
merged. Nothing told the lane so. `lucidos hardened integrated-heads` listed
only integrated children's heads, and a lane branch has none.

## Decision

- **`lucidos hardened reviewed-heads` lists every head another run certified.**
  It replaces `integrated-heads` and wraps
  `GET /api/v1/internal/reviewed-heads`. It returns the integrated children's
  heads as before. On a `lane/<batch id>` branch it also returns each member
  snapshot from `LaneBatchStarted` whose member branch holds a full marker at
  exactly that sha.
- **A lane with no marker and no own commits follows the merge-only rules.**
  `/harden` skips Phases 0.5 and 0.6, reviews each landing merge's
  `--remerge-diff` and the overlap inline, and skips the other-model review.
- **Under main-check coverage, suites follow the resolutions too.** On a first
  lane harden, `harden-scope.sh --suite-paths` selects from the lane's own
  commits, each merge's resolution files and the overlap. An empty set, a
  quoted path or any failure selects from the whole branch diff.

## Rationale

The member's own run reviewed and tested its diff at that commit. Re-running
it in the lane spends the time where nothing changed. The lane's real new
lines are its resolutions, and the overlap is where a clean merge can still
break.

Exact-sha matching keeps the claim narrow. A marker at another head, a fast
marker, a missing row and a remote member all drop out, and a dropped member
forces FULL. Every unknown widens the review, as ADR 0315 requires.

## Consequences

- A lane batch with trivial conflicts lands in minutes, not after a full round.
- A semantic break across files that merged clean can first surface after
  landing. As in ADR 0758, the main check owns that risk, and suites narrow
  only where it runs.
- Remote members from a paired gateway carry no local marker, so a batch with
  one still gets a full review.
- An old CLI paired with the new skill has no `reviewed-heads`. Phase 0.4 then
  reads no heads and answers FULL.

## Alternatives considered

- **A separate `lane-heads` subcommand.** It would leave `integrated-heads`
  naming only part of what Phase 0.4 needs, and two calls to keep in step.
- **Trust a member's marker when it is fresh against its branch tip.**
  Freshness says nothing about the commit the lane merged. A member that moved
  on and re-hardened would vouch for code the lane never merged.
- **Narrow suites without main-check coverage.** Nothing would then run the
  member's suites against the merged tree. ADR 0758 already declined that.
