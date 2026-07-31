# 0029 — A release tag names the main-line commit; the mirror's tag of the same name names the orphan

**Status:** Accepted (2026-07-30)

## Context

Lucidos publishes to two remotes:

| remote | what it holds |
|---|---|
| `lucidos` (PUBLIC MIRROR) | ONE squashed **orphan** commit per release, force-pushed to its `main`. No internal history, ever (ADR 0024). |
| `origin` (PRIVATE) | the real development history — every commit, every branch. |

A release therefore has **two** commits that legitimately represent it: the
stripped orphan on the mirror, and the "Release v\<version\>" commit on `main`
that bumps `RELEASE`, `CHANGELOG.md` and `install.sh`. Both want to be called
`v<version>`.

Until this ADR, only the first existed. `release-to-lucidos.sh` did:

```sh
git tag -f "$tag" "$commit"          # $commit = the stripped ORPHAN
git push --force "$REMOTE" "refs/tags/$tag"
```

Creating the tag locally was incidental — just how you push a tag by name — but
its consequences were not:

- **The tags named nothing.** 26 of 27 `v*` tags shared no history with `main`.
  `git describe --tags main` reported `v0.9.6-4946-gfb4b344cf`, naming a tag from
  eight releases earlier because it was the only reachable one.
- **Every `PREV_TAG` guard in `release.sh` was vacuous.** "`main` has no commits
  beyond `$PREV_TAG`" counted 14187 commits and could never fire. The
  deleted-files gate diffed `main` against a *stripped* tree — a subset of an old
  `main` — so it could never fire honestly either.
- **`main` could silently miss its own version.** `advance_local_main` only
  tried `git merge --ff-only`; when `main` moved during the 40+ minute build it
  printed a WARNING and continued. v0.17.0 hit exactly that: `main` moved at
  21:57 (OAuth work) after the 17:30 cut, never got the bump, and the site
  publisher — which reads the version from the local checkout's `RELEASE` file —
  kept serving the previous version's DMG until it was repaired by hand.
- **`origin` was never pushed at all.** Zero tags, routinely behind local `main`.

## Decision

**The same tag name points at a different object on each remote, and that is
deliberate.**

- The **local** `v<version>` tag names the **release commit on `main`**. That is
  what restores ancestry, makes `git describe` meaningful, and makes the
  `PREV_TAG` guards honest.
- The **mirror's** `v<version>` tag names the **stripped orphan**. It must: the
  GitHub Release is created at that tag and every download URL resolves through
  it.
- **`origin`'s** `v<version>` is the local one — the private remote holds the
  real history, so it gets the real commit.

Three consequences follow, and all three are load-bearing:

1. **The mirror tag is pushed by SHA.**
   `git push --force "$REMOTE" "$commit:refs/tags/$tag"` publishes the orphan
   under the tag name while creating and clobbering **no local ref**. Going
   through a local tag is precisely what caused the defect.
2. **A release LANDS the bump on `main`; it does not merely try to.** Fast-forward
   when possible, **cherry-pick the single release commit** when `main` moved,
   and **fail loudly** (after `cherry-pick --abort`) when that conflicts. The
   only remaining skips are operator-state problems — the checkout is not on
   `main`, or it is dirty — and those are reprinted in a "STILL OWED" block at
   the very *end* of the run, not buried mid-log.
3. **`origin` is pushed at publish time, and never forced.** `main` and the new
   tag both go up. A failure there is a loud post-release warning with the exact
   retry command, never an unwind: the public release is already out by the time
   this runs, and a private remote that is unreachable is a retry, not a failed
   release. A non-fast-forward `origin/main` means the maintainer has work this
   checkout has not fetched — overwriting it to finish a release would be a far
   worse bug than the one this fixes.

Implementation: `scripts/lib/release_main_sync.sh`, wired into `release.sh` as
one `settle_source_side` entry point shared by Phase B and the one-shot. Tested
offline by `scripts/lib/release_main_sync_test.sh` against throwaway repos and a
local bare "remote".

## Consequences

**The drift guards become real, and needed a filter.** Once `PREV_TAG` names a
main-line commit, the deleted-files gate compares two full **internal** trees
rather than `main` against a stripped one. Deletions of internal-only paths
(`docs/plans/**`, the release scripts) would start gating releases over files
that can never reach a user, so they are filtered out through the single shared
list — `release_tree_path_is_excluded` in `scripts/lib/release_tree.sh`. The
gate's question is "did `main` delete something the previous release *ships*?",
and a withheld path ships nowhere.

**The existing 26 orphan tags are left alone.** Retro-fixing them would rewrite
what the mirror's Releases resolve through. Instead the guards *degrade*: when
`PREV_TAG` is not an ancestor of the base ref, `release.sh` says so in one line
and treats the checks as advisory. This is not hypothetical — the FIRST release
after this change still resolves `PREV_TAG` to a legacy orphan. The one after
that is honest.

**`PREV_TAG` resolution stays a semver sort**, deliberately not `git describe`.
Describe answers "nearest *reachable* tag", which silently substitutes an older
tag exactly when the newest one is an orphan — the situation this ADR exists to
handle. The release knowhow warns against it for the same reason.

**Mode 2 (a PR release) tags the release-branch commit.** Mode 2 deliberately
does not touch `main` — its tree is based on the previous tag — so its tag is
not on the main line by construction. The pending-mode2 log is what chases
porting that content back; the degradation path above covers the tag.

**A `v<version>` tag existing locally now MEANS something.** It is only ever
created at a settled release commit, so "the tag exists" implies "this names a
real commit". When nothing landed, no tag is created and the summary prints the
commands to create it by hand — an absent tag is a visible gap, where a dangling
one was invisible for 27 releases.

## Alternatives rejected

- **One tag for both remotes (tag the orphan everywhere).** The status quo. It
  is what made `git describe` useless and every drift guard vacuous.
- **One tag for both remotes (tag the main commit everywhere).** Would break the
  GitHub Release and every published download URL, which resolve through the
  mirror's tag.
- **A differently-named local tag** (`release/v<version>`, `internal/v<version>`).
  Rejected: `git describe`, `PREV_TAG`'s semver sort, and every human typing
  `git log v0.17.0..` all key on `v*`. A second naming scheme means the obvious
  command keeps giving the wrong answer.
- **Retro-tagging the 26 legacy releases onto main.** Their main-line commits are
  identifiable, but the payoff (older `git describe` output) does not justify
  rewriting refs the public Releases resolve through. Degradation is cheaper and
  self-healing.
- **Force-pushing `origin` when it rejects.** Rejected outright: `origin` is the
  only copy of the real history.
