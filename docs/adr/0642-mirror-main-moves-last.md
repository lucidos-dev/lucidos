# 0642: The public mirror's main moves last, after the Release is published; a release tag may name a child of main while in flight

- **Status**: Accepted
- **Date**: 2026-11-09
- **Amends**: [0039: The public mirror's `main` is a linear release history](0039-the-public-mirror-is-a-linear-release-history.md) (the publish order), [0462: The public mirror has one door](0462-mirror-push-gate.md) (the tag rule)

## Context

The v0.57.0 release was cancelled after `release-to-lucidos.sh` had pushed the
`Release 0.57.0` commit to the mirror's `main` and pushed the `v0.57.0` tag. The
tarball build failed, the operator started over as v0.57.1, and the drafts were
deleted. The mirror's `main` kept the commit.

Everything that reads the mirror's `main` then advertised a release nobody could
install:

| Reader | Effect of an unpublished commit on `main` |
|---|---|
| Engine What's New (`engine/changelog.rs` reads `main`'s `CHANGELOG.md`) | Lists the release as newer and offers Update Desktop App, which the gateway refuses. |
| `install.sh` and `uninstall.sh` self-fetch from `main` | Serves an installer whose baked version has no assets. |
| Readers of the mirror | See a release commit with no GitHub Release. |

ADR 0039 chose "`main` first, then the tag" only to keep its commit and tag
count simple. Nothing needs `main` to move early: the tag push carries the
commit object, and `release-tarballs.yml` checks out the tag.

## Decision

The publish order is: push the tag by SHA, create the DRAFT Release, upload,
wait for the tarballs, publish, and only then lease-push `main`. While that is in
flight, the `v<version>` tag may name a direct child of the mirror's `main`.

## Rationale

`main` is the mirror's public statement of what is released. Once it moves only
after the Release is published, it never names a release that cannot be
installed. That fixes every engine and installer already in the field, with no
engine change, because they all read `main`.

The tag is the right thing to lead. A tag without a published Release is
invisible to every reader above. It is also the only ref the tarball workflow
needs.

## Consequences

- **The transient state flips.** It used to be "`main` holds one commit no tag
  names". It is now "one tag names a child of `main`". In `release_tree.sh`:
  - `release_mirror_history_check` takes the in-flight tag name, not a count.
    `release_mirror_in_flight_count` excuses it only while it names a direct
    child of `main`, and the completeness check subtracts it from the tags.
  - `release_mirror_tags_are_on_main` skips only that tag, under the same
    condition. Any other tag off `main` still refuses.
  - The one door (ADR 0462) admits a `v*` tag only on published ancestry: its
    parent is the mirror's `main`, or it is parentless when there is no `main`.
- **`release_mirror_main_advance` is the last step on every publish path.** It
  never rewinds: when `main` is or already holds the commit, it does nothing.
  So a retry after a failed `main` push only advances `main`.
- **Nothing reserves `main` before the publish, so racing releases recount.**
  The old order leased `main` first, which serialized two releases. Now each run
  pushes its tag, then re-runs the history check. A rival's tag makes the count
  one too many, so the run deletes the tag it created and refuses. At most one
  of two racers passes; both may back off.
- **A cancelled release leaves nothing public.** `release.sh --abandon` deletes
  the `v<version>` draft Release, then the tag through
  `release_tree_delete_release_tag`. It refuses when the GitHub Release is
  published or its state is unreadable, and keeps the tag while a Release
  survives. The library refuses a tag that `main` holds.
- **A failure between publish and the `main` push leaves `main` one release
  behind.** That is the safe direction: What's New shows nothing new until the
  retry, rather than an update that cannot install.
- **The existing `Release 0.57.0` commit stays.** Rewriting published history
  breaks clones (ADR 0039).

## Alternatives considered

- **Gate What's New in the engine on a published Release.** It fixes only
  engines that take the update, and leaves `install.sh` and the mirror itself
  wrong. The pipeline fix covers every reader at once.
- **Keep the order and roll `main` back on cancel.** That is a force-push of
  published history, and a crash before the rollback leaves the same state.
- **Push `main` right after the tag, before the Release is published.** The
  tarball wait is the step that failed for v0.57.0, so `main` would still lead
  an unpublished release.
