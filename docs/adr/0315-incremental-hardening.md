# 0315: After a hardened branch gains commits, /harden reviews only what no hardening has reviewed

- **Status**: Accepted
- **Date**: 2026-09-28
- **Amends**: [0295: After a merge, /harden reviews only the resolution and the overlap](0295-merge-only-hardening.md)

## Context

ADR 0295 made a merge session review only the resolution. It accepted one
cost: "A Phase 4 fix in a merge-only run is a commit of the branch's own. The
next pass takes the full procedure."

That cost landed on the first real merge with a failing suite. The conflict
took 44 seconds to resolve. The suites then found a test that failed on `main`
too, and the session fixed it. The fix sent `/harden` back to a full review.

Codex re-read the whole feature and raised two edge cases in code the earlier
run had hardened. Each fix cost another full suite run. The session ran for
more than 35 minutes, in four suite rounds.

The failing test had its own cause. A Rust-only change added a `SystemEvent`
variant. `harden-suites.sh` selected only the Rust suite, but
`sse-event-coverage.test.ts` reads that Rust file. So `main` went red unseen,
and two threads fixed it in parallel. Their fixes then conflicted.

The plan is `docs/plans/2026-09-28-incremental-hardening-and-main-red-guard.md`.

## Decision

`scripts/harden-merge-scope.sh` becomes `scripts/harden-scope.sh`. Beside
`MERGE_ONLY` and `FULL` it answers `INCREMENTAL` when the branch has commits of
its own since the hardened SHA. It lists those commits, any merges and their
overlap. The review covers exactly those commits' patches and the merges'
resolutions. Codex reviews against the hardened SHA when no merge is in range,
and is skipped otherwise. Every selected suite still runs.

`harden-suites.sh` selects the Vitest suite for any tracked file that a Vitest
test names by path. That list comes from a scan of the test sources on each
run. Harden Phase 4 also merges `main` again before fixing a failure that
fails on `main`.

## Rationale

ADR 0295's argument holds for any commit, not only for merges. The earlier run
reviewed every line up to the hardened SHA. Git lists exactly what came after
it, so the check still needs no judgment. The cost it accepted was a choice of
mechanism, not a limit of the argument.

Codex takes one base. With no merge in range, `<sha>...HEAD` is exactly the new
code. With a merge in range, that range also holds all of main's new code,
which other hardenings already reviewed. Skipping Codex there matches
merge-only mode.

The Vitest scan fixes the cause of the red `main`, not the symptom. A
hand-kept list of cross-layer files goes stale the day a test starts reading a
new one. The scan cannot, and it also replaces the one hand-written entry
(`sdk_iframe.css`).

## Consequences

- A fix during a merge session costs one short review of that fix, not a
  full review and a Codex round on the whole branch.
- A real bug in already-hardened code is no longer found by the merge
  session's re-review. The two Codex findings in the worked case were real.
  That is the same trade ADR 0295 made for merges: the earlier run owned
  that code.
- A merge of a branch other than `main` now answers `FULL` by its own check:
  its commits are listed nowhere. ADR 0295 reached `FULL` there only by
  accident, through the own-commit rule.
- The scan finds only whole path literals. A test that builds a read path
  from pieces at run time is missed. A directory literal selects every file
  under it, which errs toward running Vitest, a suite of about 20 seconds.

## Alternatives considered

- **Keep ADR 0295's full re-review after a fix.** It is what turned a
  44-second conflict into four suite rounds.
- **Run Codex against the hardened SHA even with a merge in range.** Its
  diff would hold all of main's new code as the branch's, and bury the few
  new lines.
- **Run Vitest for every Rust change.** Simple, but it misses the same class
  for non-Rust files a test reads, such as rules and CSS, and still needs the
  `sdk_iframe.css` rule.
- **Leave a failure that also fails on `main` to someone else.** `/harden`
  cannot pass while it fails, so the branch stalls. Merging `main` again first
  catches the common case, a fix that landed minutes ago.
