# 0039: The public mirror's `main` is a linear release history, and a parent is safe because the mirror already publishes it

- **Status**: Accepted
- **Date**: 2026-08-04
- **Relates to**: [0024: The release candidate IS the published artifact](0024-the-release-candidate-is-the-published-artifact.md), [0029: A release tag names the main-line commit](0029-a-release-tag-names-the-main-line-commit.md)

## Context

`github.com/lucidos-dev/lucidos/commits/main` showed **one commit**. Verified
against the live remote on 2026-08-04, before this change:

| | |
|---|---|
| `git rev-list --count` on `lucidos/main` | **1**, subject `Release 0.20.1`, no parents |
| Tags on the mirror | **36**, all lightweight, all `v*` |
| Relationship between any two of them | none. 36 unrelated parentless commits |

That was the design, not a defect in it. `release_tree_commit` ran
`git commit-tree` with no `-p`, and both publish paths force-pushed that
parentless object over whatever `main` held. `release_tree_is_orphan` actively
**refused** a candidate with parents, and its reason was real:

> A push sends every object reachable from the commit, while `release_tree_scan`
> only ever inspects the **tip tree**. A candidate with ancestry would put
> objects on the mirror that the private-data guard never looked at.

That argument is correct. What was wrong was the conclusion drawn from it. "No
parents at all" is a sufficient condition for "nothing unscanned is published",
not a necessary one, and it was bought at a high price:

- **The published history was a lie by omission.** 36 releases, one commit.
  Nobody reading the mirror can see that v0.12.4 came before v0.13.0, or diff
  two releases, or `git log` the project at all.
- **Every release broke every clone.** A parentless force-push is never a
  fast-forward, so any existing clone of the mirror needed a reset to follow.
- **Every tag named an island.** `git describe`, `git merge-base` and every
  other ancestry question were meaningless on the mirror.

A one-time repair (`scripts/rebuild-mirror-history.sh`, 2026-08-03) was written
to rebuild the published history as a chain. It was never enough on its own:
its own plan says chaining FUTURE releases is a separate task, so the next
release would have force-pushed a fresh parentless commit over the repaired
history and put it straight back to one commit.

## Decision

**The mirror's `main` is a linear history of releases. Release N's published
commit carries release N-1's published commit as its single parent.**

Four parts, and the first is the one that makes the rest safe.

### 1. The guard changes shape; it is not dropped

`release_tree_is_orphan` becomes
`release_tree_ancestry_is_published <repo> <commit> <expected-parent>`, and it
asserts the precise property the blunt rule was standing in for:

- **at most one parent.** Two is a merge, and the published history is strictly
  linear, so a merge is a refusal rather than a special case.
- **that parent must be exactly the SHA the mirror's own `main` holds.** A child
  of the mirror's `main` adds **no object the public does not already have**, so
  the scan gap the orphan rule existed to close stays closed.
- **no parent is accepted only when the mirror has no `main`.** A parentless
  candidate against a mirror that *does* have history would silently discard
  every published release, which is precisely the old behaviour.

Each refusal names which of the three it is, because the operator's next move
differs: a merge is a bug in the pipeline, a wrong parent means re-run
`--push-rc`, and a missing parent means the candidate predates this ADR.

### 2. The parent is baked in at Phase A and never re-parented

ADR 0024 says the release candidate **is** the published object: what CI gated
on is what ships. The parent is part of that object's identity, so it is
resolved once, in Phase A, from the mirror's `main`, recorded as `RC_PARENT`
beside `RC_COMMIT` in `verify-build-<version>.env`, and never recomputed.

If a release lands between Phase A and Phase B, Phase B **refuses** and names
`release.sh --push-rc <version>`. That rebuilds the candidate onto the new tip,
which produces a **different object**, which correctly re-fires the CI gate. The
old verdict genuinely does not carry over: a different parent is a different
history, and the gate tested the other one.

### 3. The `main` push is a leased compare-and-swap, and stays idempotent

`--force-with-lease=refs/heads/main:<RC_PARENT>` moves the precondition to the
**server, at update time**, where the window between Phase B's read and the push
cannot be raced by the confirmation prompt a human may sit in front of for
minutes.

A lease alone would break re-runs, because the first successful push moves
`main` off the recorded parent and the second run would refuse itself. So the
decision is made first, by the pure `release_main_push_decision`:

| mirror `main` is | decision | what happens |
|---|---|---|
| the candidate | `published` | already out; push nothing, stay idempotent |
| the recorded parent | `lease` | `--force-with-lease` against that SHA |
| absent, with no parent recorded | `create` | plain create; nothing to lease against |
| anything else | `refuse` | a release landed since; publishing would drop it |

### 4. The history must account for every release, and Phase A enforces it

Chaining onto a one-commit `main` only ever produces a two-commit `main`, so the
historical repair has to land first. Rather than rely on remembering that,
Phase A refuses when `git rev-list --count` on the mirror's `main` does not equal
the number of published `v*` tags, and the refusal names
`scripts/rebuild-mirror-history.sh`.

The count is **necessary but not sufficient**, so a second half runs after it:
`release_mirror_tags_are_on_main` asserts every published `v*` tag really is an
ancestor of `main`. A tag rewritten onto an unrelated commit, or one deleted
while another is added, keeps the totals equal while the history stops
accounting for the releases it advertises, and a guard named after that property
has to actually test it. It is affordable because it needs no extra fetch and
creates no local ref: counting `main` already fetched it, and a fetch brings
everything reachable, so a tag whose object is still absent is by that fact not
on `main`. Fetching the tags themselves is deliberately avoided, because
`git fetch --tags` would plant the mirror's stripped commits under local `v*`
names, which is precisely the ADR 0029 regression. The count runs first, so the
pre-repair state still reports "run the repair" rather than 36 stray tags.

The check is **permanent, not a one-time step**. Before the repair it is 1
against 36 and refuses. After it, 36 against 36. Every release thereafter adds
exactly one commit and one tag, so it self-maintains, and it refuses in the other
direction too: more commits than tags means something reached `main` that no
release published, which nothing in the pipeline can do.

### 5. Both publish paths are held to all of this, including the legacy one-shot

The first cut of this change guarded only Phase A, and the legacy one-shot
(`release.sh <version>` with no phase flag, or a hand-run of
`release-to-lucidos.sh`) broke the invariant two separate ways. Both are worth
recording, because both were silent and both corrupted exactly what parts 1 to 4
exist to protect.

- **It skipped the completeness precondition**, so the one path with no rc gate
  was also the one path that could publish onto the unrepaired mirror, leaving
  2 commits against 37 tags and staling `rebuild-mirror-history.sh`'s pinned
  total. `release_mirror_history_check` is now a single shared function that
  both paths call. A precondition only one of two doors checks is not one.
- **It lost its idempotency to the parent.** That path had no recorded
  `RC_COMMIT` to fall back on: its re-runnability came *entirely* from the
  commit being parentless and deterministic, so a retry after a partial publish
  (tag pushed, Release creation or upload failed) rebuilt the identical object
  and the push was a no-op. With a parent, a retry reads the commit it just
  published as its own parent and builds a **second commit for the same
  version**. The mirror then carries more commits than release tags, and every
  later release refuses on the check above.

  The fix is the same idiom Phase A already uses for a matching rc: a version
  the mirror **already publishes with an identical tree is adopted, not
  rebuilt**. A tag that exists with a *different* tree is refused outright,
  because re-releasing one version with different content leaves the tag, the
  Release page and every download URL disagreeing; the answer there is a version
  bump, not a force-push.

- **And the completeness check had to learn about the window between the two
  pushes.** Publishing is `main` first, then the tag. If the tag push fails,
  `main` legitimately carries one commit no tag names. Ordered naively, the
  retry whose *only remaining job* is to push that tag arrives at 37 commits
  against 36 tags and is refused by the check, for exactly the state it exists
  to repair, with no in-workflow escape and every later release refused too.
  That deadlock is strictly worse than the stray-hand-push case the second arm
  was guarding against.

  So `release_mirror_history_is_complete` takes an **in-flight** count, the
  adoption is resolved **before** the check rather than after, and the one-shot
  passes 1 only when it has proven `main` is its own untagged commit (main's
  tree equals the tree being published, and this version has no tag). The tree
  is a sound discriminator because a release commit always bumps `RELEASE`,
  `CHANGELOG.md` and `install.sh`, all of which are in the published tree, so
  two consecutive releases can never share one. The input is **required, not
  defaulted**: defaulting to 0 is the deadlock, and defaulting to 1 would
  blanket-excuse a genuinely stray commit.

## Consequences

- **The mirror reads as a project.** `git log`, `git diff v0.19.0..v0.20.1`, and
  `git describe` all work on it for the first time.
- **A release is a fast-forward, so clones stop breaking.** The lease still
  makes it a `--force-with-lease` push mechanically, but the update itself is
  ancestry-preserving.
- **The mirror's disk footprint grows** by one commit object and one tree per
  release. The trees already existed as tag targets, so this is negligible.
- **A release now depends on reading the mirror.** Phase A cannot build a
  candidate without knowing `main`, so a network failure against the mirror
  refuses the release rather than silently producing a parentless one. That is
  deliberate: the failure mode of guessing is the one this ADR exists to end.
- **The one-time repair is still required, once, by a human.**
  `scripts/rebuild-mirror-history.sh --push` rewrites 36 tags and `main`
  atomically under a lease, with a rollback bundle and a typed confirmation.
  Until it runs, part 4 refuses every release.
- **`release_promote_preflight` gained an arity check.** Its parent pair was
  appended to an existing five-argument signature, and a caller left at five
  would have read two empty strings and skipped the parent half in silence. A
  guard that quietly stops guarding is the failure this whole area is written
  against, so the wrong arity is a refusal.
- **ADR 0029 is unchanged.** The mirror's `v<version>` still names the stripped
  published commit and is still pushed **by SHA**, touching no local ref; the
  local and `origin` tag still names the release commit on the internal `main`.

## Alternatives considered

**Fold the rebuild into the release flow** (so the next release both repairs and
extends the history). Rejected. Rewriting 36 tags is destructive, and the repair
script earns its safety from a dry run, byte-for-byte object verification, an
atomic leased push, a typed confirmation and a rollback bundle. Running it
inside an irreversible release would dissolve every one of those, and it would
put a 36-ref rewrite on the critical path of an operation that must be able to
fail safely.

**Derive the parent from a semver walk of the tags** rather than from `main`.
Rejected as strictly more machinery for the same answer: `main` *is*, by
definition, what the previous release published, one `ls-remote` answers it, and
the same SHA is then directly usable as the push lease. A semver walk would also
have to decide what to do when `main` and the newest tag disagree, which is a
question part 4 answers better by refusing.

**Push `main` and the tag atomically** in one `git push --atomic`. Rejected:
the two refs have different idempotency semantics. `main` is leased and
skippable when already published; the tag is an unconditional by-SHA force that
must stay re-runnable. Binding them would make a retry after a partial success
unexpressible.

**Keep the parentless commits and accept the one-commit mirror.** This was the
status quo and it is what the question that prompted this change was about. The
cost was never a cosmetic one: a public repository whose history cannot be read
is a worse artifact than one that can, and the force-push broke every clone on
every release.
