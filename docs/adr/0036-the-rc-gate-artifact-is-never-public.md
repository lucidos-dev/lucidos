# 0036: The release-candidate gate artifact is never public: a DRAFT release, fired by dispatch (drafts emit no `release` event)

- **Status**: Accepted
- **Date**: 2026-08-03

## Context

Phase A of `scripts/release.sh` arms the clean-machine DMG gate by putting the
staged, signed, notarized DMG on an `rc-<version>` release in the public mirror.
Until now that release was a **prerelease**, and a prerelease is publicly listed:
for the whole Phase A to Phase B window it sat at the top of
github.com/lucidos-dev/lucidos/releases, above the current GA. On a deferred
notarization (ADR 0027) that window is days. On 2026-08-03 `rc-0.19.1` was
visible there and had to be deleted by hand.

The window exists because the gate is armed at a different time from when the
release is published, which is the entire point of the two-phase flow (ADR 0024).
Shrinking it is not a fix; the artifact should never have been public at all.

Nothing but the gate consumes it. `dmg-verify` does `gh release download "$TAG"
--pattern '*.dmg'`; `front-door` tests the `rc.lucidos.dev` origin rather than
the release; `tarball-smoke` builds its own tarball; and Phase B reads the DMG
from the on-disk staging dir and gates on the rc BRANCH sha. So the rc release is
a private transport from the release Mac (where the signing identity lives) to a
clean CI runner, and it was public only as an accident of how the gate was fired.

## Decision

**The `rc-<version>` release is created as a DRAFT** (`gh release create --draft
--prerelease`). A draft is invisible on the public releases page and creates no
tag ref at all.

**The gate is fired by an explicit `workflow_dispatch`, never by a `release`
event.** After creating the draft, Phase A runs `gh workflow run
install-smoke.yml -f dmg_tag=rc-<version>`.

**`dmg-verify` declares `permissions: contents: write`**, which is what lets
`github.token` see a draft at all.

**The name "release candidate" is unchanged.** It names the role (the object
promoted to GA), not the visibility, and the rc BRANCH stays public. Only the
word "prerelease" is retired in favour of "draft release", since the artifact is
no longer a prerelease.

## Why not fire it from the `release` event

This is the part worth not re-deriving. The obvious move is to add `created` to
the workflow's `release: types:` and accept `github.event.release.draft == true`.
**It cannot work.** GitHub's Actions documentation is explicit:

> Workflows are not triggered for the `created`, `edited`, or `deleted` activity
> types for draft releases.

Creating a draft emits no workflow-triggering event whatsoever, so there is no
`types:` entry and no `if` expression that catches it. (The same page adds that
`prereleased` does not fire for a pre-release published from a draft, either.)

Adding `created` would also do positive harm: it DOES fire for a non-draft
release, so the older-style rc prerelease that the retained trigger arm still
supports would match both `created` and `prereleased` and run `dmg-verify` twice.

The dispatch route needed no new plumbing, because ADR 0027 already built it for
the deferred-notarization case: the `dmg_tag` input exists, `dmg-verify`'s `if`
already accepts `workflow_dispatch && inputs.dmg_tag != ''`, and every other job
in the file guards on `inputs.dmg_tag == ''` so a gate dispatch starts that job
and nothing else. `release.sh` already had `dispatch_dmg_verify <tag>`.

## Why `contents: write` on that job

GitHub's REST documentation for listing releases: *"Only users with push access
will receive listings for draft releases."* The mirror's
`default_workflow_permissions` is `read`, so the job's token could not resolve a
draft by tag without an explicit grant.

The gh-side obstacle is gone. cli/cli#3037, "Accessing draft releases is not
possible using GITHUB_TOKEN in Actions", was fixed by PR #3656 (merged
2021-05-18): the CLI dropped its write-access probe (which read null for a
server-to-server token) and now looks drafts up unconditionally via GraphQL
`repository.release(tagName:)`, in parallel with the published-release lookup.

The grant is job-scoped and the token is step-scoped (`env: GH_TOKEN` on the
download step alone), so the DMG that the job mounts and launches never sees it.
`front-door`'s `permissions: {}` must NOT be copied onto `dmg-verify`: an empty
map is exactly the state that cannot read the draft.

## Consequences

- The public releases page shows GA releases only, at every point in a release.
- No `rc-<version>` **tag ref** is created either, so the mirror's tag list stays
  clean without a cleanup step.
- Cleanup can no longer pass `--cleanup-tag` blindly. `gh release delete
  --cleanup-tag` deletes the release and THEN deletes the tag ref, turning the
  404 for a draft's absent ref into a non-zero exit AFTER the release is already
  gone. `rc_release_delete` in `release.sh` deletes the release, removes the tag
  ref only when one exists (covering legacy non-draft rcs), and reports failure
  only if the RELEASE survived.
- A dispatch that cannot be queued is fatal to the arming step, with the
  `--push-rc` retry hint, on the same reasoning as a failed `gh release create`:
  a silently unarmed gate looks exactly like a passing one, right before an
  irreversible publish.
- The `release: prereleased | released` arm of `dmg-verify` is deliberately kept,
  so a hand-made rc prerelease, or one created by an older `release.sh`, still
  fires the gate.
- Escape hatch if a draft ever proves unreadable from CI: `gh release edit
  rc-<ver> --repo lucidos-dev/lucidos --draft=false` restores exactly the old
  behaviour, at the cost of the public listing.

## Relationship to ADR 0027

ADR 0027 rejected "keep `rc/<version>` and its prerelease alive past GA so
`dmg-verify` can fire the usual way" on the grounds that a prerelease sitting
above the GA release on the public releases page is unacceptable. That reasoning
was right and is now addressed at the source: the gate artifact is not public in
the first place. 0027's chosen remedy for the deferred case (follow the artifact
to the GA tag with `--attach-notarized`, dispatching `dmg_tag=v<ver>`) is
unchanged, and is the same mechanism this ADR now uses for the rc.
