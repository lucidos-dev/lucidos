# 0791: Version status states in `engine_version.rs` carry the facts a client needs

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0617](0617-engine-build-delivery-proof.md)

## Context

The engine reports a version status to clients. The status toast and the
new-version confirm read it. Both need more than a bare state name to say
something the user can act on.

This record covers how `engine_version.rs` models that status and the commits
behind it.

This record restates the design comments of
`crates/lucidos-engine/src/engine/engine_version.rs`, written before the
comment strip removes them. Nobody checked it line by line against the code.

## Decision

- `Building` carries the instant the background `cargo build` started. The old
  engine keeps serving while it runs.
- `Failed` carries why the rebuild failed. The old engine keeps running.
- The delivered state carries the HEAD whose delivery was proved.
- A specific recognized failure is recorded as surviving a retry only when both
  halves hold: it is a recognized shape, and it was observed to survive a
  retry.
- Pending commits are grouped by `CommitGroupKind`, derived from the
  conventional-commit type. `CommitGroupKind::Other` holds subjects with no
  recognized type.
- The pending list counts only what a switch adds, and excludes merges through
  `--no-merges`.
- One gate answers whether an engine may report a version newer than the one it
  runs. Packaged engines answer no.
- The shared build lock probe is dev only and always false packaged.

## Rationale

**Facts live in the variant.**

- `Building` holds its start instant so the toast can count up. This makes
  "building, but nobody knows since when" unrepresentable.
- `Failed` holds its cause for the same reason. A failure no recognizer names
  still reports its exit status and last output line.
- The delivered state holds the proved HEAD, rather than assuming a successful
  script built everything present when it finished.
- `Building` is in-memory only. A build dies with the engine, and a restart
  starts from `Idle`.

**Both halves of a retry-proof failure.** Each half answers a way the other is
wrong.

- Repetition alone over-fires, because a summary is not an identity. Two
  unrelated type errors both read `error[E0308]: mismatched types`, so a user
  fixing one and meeting the next would lose the button mid-progress.
- The shape alone over-fires too. A build script reporting a missing file may
  be missing a real input, which a rebuild fixes once it is back.

**Commit groups.** `CommitGroupKind::Other` is the honest home of a hand commit
or a revert. Guessing would be worse, and dropping it would lose a commit that
might be the interesting one. The frontend owns the wording each kind renders
as. The index of a kind is an exhaustive `match`, so adding a variant is answered at compile time. The other answer is a panic on a `position(...).unwrap()` inside a version-status read.

**Only what the switch adds.** An Apply lands as a merge named after its
branch, which describes no work. What it merged is already in the range under
its own subjects. The list leaves out whatever the served client already
carries. It says "commits", not "changes", because a change is the coding-agent
change the user Applies, and a hand commit or revert is not one.

**One gate.** Three probes share the newer-version gate:
`LucidosEngine::source_behind_head`, the on-disk binary check, and the
served-frontend peer sync. Packaged engines learn of new versions from the
release updater instead.

**Shared build lock.** Co-located workspaces share one `target/` and one build
lock, so a peer's build advances the binary this engine serves. A workspace
that lost the lock would otherwise read as idle and wrongly offer the manual
Rebuild escape hatch instead of the spinner. The probe is named "shared"
because it cannot tell this engine's own held lock from a peer's. The frontend
disambiguates through `build_state`.

## Consequences

- Clients can always show when a build started and why a build failed.
- A user mid-progress through unrelated errors keeps their rebuild button.
- A commit with no recognized type still shows, under `Other`.
- A new `CommitGroupKind` variant fails to compile until its index is defined.
- The same pending list serves the status toast and the new-version confirm,
  though each describes it differently: what is being built, and what the
  switch would bring.
- Packaged installs never report a newer version through these probes.

## Alternatives considered

- Assuming a successful script built everything present when it finished.
  Rejected in favor of carrying the HEAD whose delivery was proved.
- Recording a failure as retry-proof from repetition alone, or from its shape
  alone. Both over-fire.
- Looking up a kind's index in the order array with `position(...).unwrap()`.
  Rejected for an exhaustive `match`.
- Guessing a type for an unrecognized subject, or dropping it. Rejected for
  `CommitGroupKind::Other`.
- Including merge commits in the pending list. Excluded with `--no-merges`.
