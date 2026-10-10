# 0295: After a merge, /harden reviews only the resolution and the overlap, and still runs every selected suite

- **Status**: Accepted
- **Date**: 2026-09-26
- **Amended by**: [0315: After a hardened branch gains commits, /harden reviews only what no hardening has reviewed](0315-incremental-hardening.md). A fix no longer sends the next pass to a full review.

## Context

A hardened branch whose Apply conflicts with `main` gets a merge session. That
session merges, resolves, and runs `/harden` again. The marker was only `FRESH`
or not, so the merge commit sent `/harden` back to a full review. Four
reviewers re-read a branch diff that no line of the resolution had changed.

The merge prompt then asked for `make test` and `npm test` on top. `/harden`
had already run the suites its diff selects. So this ran them twice, and ran
the Rust suite for branches with no Rust in them.

The worked case was a CSS and TypeScript branch. Resolving its one conflicted
hunk took 20 seconds. The session after it ran for more than 7 minutes, longer
than building the feature. The plan is
`docs/plans/2026-09-26-merge-aware-hardening.md`.

## Decision

`/harden` Phase 0.4 asks `scripts/harden-merge-scope.sh` whether every commit
since the hardened SHA that is not on `main` is a merge. If so, the review
covers only the resolution (`git show --remerge-diff` per merge) and the files
both sides changed. Every suite `harden-suites.sh` selects still runs.

The merge prompt and the in-session Apply path no longer name any test
command. `/harden` owns test selection.

## Rationale

The earlier run reviewed every line the branch wrote, and no commit of the
branch's own has landed since. The only new text is what the resolver wrote.
`--remerge-diff` shows exactly that, against git's own conflicted result, and
it includes edits to files that merged clean. So the short review reads every
line nobody has reviewed yet.

A clean merge can still break meaning: main renames what the branch calls.
Review of the resolution cannot see that, so the regression angle reads the
overlap files. The suites are the stronger net, and they stay whole, as
`CLAUDE.md` requires for a merge that mixes sides.

The check fails closed. No SHA, a SHA off HEAD's history, a commit of the
branch's own, or any git error all answer `FULL`. A wrong `MERGE_ONLY` would
skip review of unreviewed code. A wrong `FULL` only costs time.

## Consequences

- The engine's hardened-state endpoint returns the recorded `head_sha`, and
  `lucidos hardened sha` prints it. `lucidos hardened query` is unchanged.
- A Phase 4 fix in a merge-only run is a commit of the branch's own. The next
  pass takes the full procedure. That is the price of a check with no judgment
  in it.
- A merge of any branch other than `main` brings in commits `main` lacks, so
  it takes the full procedure.
- A Tier-3 temp merge worktree on a differently named branch finds no marker,
  and takes the full procedure.
- A session that ends abnormally proposes its change from the session-end
  path, which consumes the marker. Its later merge takes the full procedure.
  A clean idle keeps the marker, so the common case gets the short review.
- The Codex review never reads the resolver's lines. Only `code-review` and
  the three angles do.

## Alternatives considered

- **Keep the merge prompt's fixed test step.** It duplicates Phase 4.5 and
  picks the wrong suites. Nothing it runs is missing from `/harden`.
- **Review the merge commit's first-parent diff** (`git diff HEAD^1 HEAD`).
  That is all of main's new code, most of it reviewed when it landed. It is
  large, and it hides the few lines the resolver wrote.
- **Skip `/harden` after a conflict-free merge.** The suites are what catch a
  semantic conflict, and skipping them removes the one net that works.
- **Judge "the branch did not really change" from the diff size.** The
  small-diff tier already does that for new work. For a merge the question is
  about history, and git answers it exactly.
