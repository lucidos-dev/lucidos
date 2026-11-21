# 0703: A release tag needs a clean blocker ledger, read from the repo

- **Status**: Accepted
- **Date**: 2026-11-15

## Context

v0.39.0 was tagged and published while a critical clipboard defect was marked
"must close before the next tag". The fix shipped in v0.39.1. No step on the
release path read anything that could have stopped the tag.

The work tracker held the knowledge. It records items as `WorkItemsChanged`
events in one workspace, with a hand-typed status. Many critical items there
say `done` with no commit recorded, and one fixed item still says
`in_progress`.

## Decision

1. **Release blockers live in `release-blockers.tsv`, a tracked file at the
   repo root.** One row per blocker: `id`, `state`, `ref`, `work_item`,
   `title`, `reason`. Rows stay after they close.
2. **The gate reads the ledger at main HEAD and judges it against the release
   commit.** `open` refuses. `fixed` passes only when its commit is in the cut.
   `withdrawn` passes. `waived` passes only for the one version it names.
3. **Every input error refuses.** A missing ledger, a malformed row or an
   unresolvable commit fails closed.
4. **The owner override is a `waived` row on main**, with a version and a
   reason. Git records who wrote it. The release output, the `STILL OWED`
   block, the local tag message and a `ReleaseGateChecked` event repeat it.
5. **The ledger never ships to the public mirror.** The stripped commit carries
   a `Release-Gate:` trailer with counts and a digest only.
6. **The gate runs at every door:** Phase A, Phase B, `--publish-draft`, the
   legacy one-shot, `release_tree_push` for `rc/*` and `v*`, the nightly
   `--prep-preflight`, and a `release-gate` job in `release-tarballs.yml`.

## Rationale

Git records release knowledge across workspaces and supports checks against a frozen cut.
The mirror needs only an opaque summary, so security details remain private.

## Alternatives considered

- **Query the workspace event store through the `lucidos` CLI.** The work item
  is an app concept, not an engine one (work item model, decision 9). A repo is
  developed from several workspaces, and each has its own store. The GitHub
  runner has no store at all. The status field is hand-typed, so the gate would
  block on stale criticals. `WorkItemsChanged` is also being replaced.
- **A `lucidos release-gate` engine or CLI command.** Lucidos's own release
  steps stay out of the product (work item model, decision 14).
- **A `--waive <id>` flag on `release.sh`.** The model that runs the release
  could pass it, and it would leave no record in git.
- **Ship the ledger to the mirror.** An open security blocker would be public
  before its fix.
- **Read the union of the release commit's ledger and main's.** It cannot
  express "fixed, but after the cut". Requiring the fix commit to be an
  ancestor of the cut can, with one file.

## Consequences

- Recording, fixing, withdrawing or waiving a blocker is a commit. It reaches
  main through Apply, like any other change.
- A blocker opened after the cut refuses the tag. The answer is a fix and a new
  cut, or a waiver for this version. The gate never re-folds (ADR 0639).
- The tracker can show ledger rows from `ReleaseGateChecked`. The gate never
  reads the tracker.
- Another factory adopts the gate by adding the file and calling
  `release_blockers_check` from its own release step.
- Who asked for a waiver is policy, not proof: git cannot tell an owner's
  instruction from an agent's own choice. Every waiver is printed where the
  owner reads the release.
